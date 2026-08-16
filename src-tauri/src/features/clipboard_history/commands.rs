//! 剪贴板历史命令（薄壳：参数解析 + 状态获取，业务逻辑在 service / store）。
//!
//! 命令均为异步（内部等待插件异步 API）。所有业务动作由前端发起，
//! 后端无自有时钟（见 `docs/adr/0001-clipboard-capture-via-webview-events.md`）。

use super::service::{
    dedup_promote_and_evict, evict_over_limit, orphan_files, set_favorite, sort_for_display,
    CleanupResp, ClipboardEntry, ClipboardFiles, ClipboardImage, InsertOutcome, SetMaxResp,
    MAX_ENTRIES_LIMIT,
};
use super::store::{HistoryStore, StoreBackend};
use crate::core::error::ApiError;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// 插件保存图片的目录（插件默认路径，相对应用数据目录，见契约 5.4）。
/// 分开 join 两个组件，避免单个含 `/` 的字符串在 Windows 上保留正斜杠、
/// 与插件落盘路径（`\`）不一致导致孤儿清理误判（见 service::orphan_files 注释）。
const IMAGE_DIR_SUB: &str = "tauri-plugin-clipboard-x";
const IMAGE_DIR_NAME: &str = "images";

const ERR_CAPTURE: &str = "clipboard.capture_failed";
const ERR_ENTRY_NOT_FOUND: &str = "clipboard.entry_not_found";
const ERR_INVALID_MAX: &str = "clipboard.invalid_max_entries";

/// 捕捉互斥锁：主窗口与小窗（快速粘贴 popup）可能同时发起 `capture_clipboard`，
/// 锁内串行化「读 store → 去重置顶 → 写 store」，避免并发读到同一状态导致重复插入。
/// 仅保护同步的 store 段（无 await），读剪贴板 IO 在锁外进行。
static CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn capture_err(err: impl std::fmt::Display) -> ApiError {
    let code = ERR_CAPTURE.to_string();
    log::error!("{code}: {err}");
    ApiError::new(code, format!("read clipboard failed: {err}"))
}

fn entry_not_found(id: &str) -> ApiError {
    log::error!("{ERR_ENTRY_NOT_FOUND}: {id}");
    ApiError::new(ERR_ENTRY_NOT_FOUND, format!("entry not found: {id}"))
}

fn image_dir(app: &AppHandle) -> Result<PathBuf, ApiError> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| ApiError::new("clipboard.storage_error", e.to_string()))?;
    Ok(base.join(IMAGE_DIR_SUB).join(IMAGE_DIR_NAME))
}

fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn list_png_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read_dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "png"))
        .collect()
}

fn delete_files(paths: &[PathBuf]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

/// 读取剪贴板各格式（逐格式独立容错，契约 5.2-1）。
///
/// 返回 `(条目, 本次 `read_image` 落盘的图片路径列表)`。图片在本函数内提前落盘（插件行为），
/// 但后续去重可能丢弃该图片（例如命中一个不含该图的旧条目），因此调用方需根据返回的路径
/// 清理未最终引用者，避免产生孤儿文件（见 `capture_clipboard`）。
async fn read_clipboard(app: &AppHandle) -> (Option<ClipboardEntry>, Vec<String>) {
    let mut entry = ClipboardEntry {
        id: uuid::Uuid::new_v4().to_string(),
        captured_at: now_iso8601(),
        favorited_at: None, // 新捕捉默认为非收藏（契约 5.8）
        text: None,
        html: None,
        rtf: None,
        image: None,
        files: None,
    };
    let mut written_images: Vec<String> = Vec::new();

    if tauri_plugin_clipboard_x::has_text().await.unwrap_or(false) {
        match tauri_plugin_clipboard_x::read_text().await {
            Ok(text) => entry.text = Some(text),
            Err(e) => log::warn!("read_text failed: {e}"),
        }
    }
    if tauri_plugin_clipboard_x::has_html().await.unwrap_or(false) {
        match tauri_plugin_clipboard_x::read_html().await {
            Ok(html) => entry.html = Some(html),
            Err(e) => log::warn!("read_html failed: {e}"),
        }
    }
    if tauri_plugin_clipboard_x::has_rtf().await.unwrap_or(false) {
        match tauri_plugin_clipboard_x::read_rtf().await {
            Ok(rtf) => entry.rtf = Some(rtf),
            Err(e) => log::warn!("read_rtf failed: {e}"),
        }
    }
    if tauri_plugin_clipboard_x::has_image().await.unwrap_or(false) {
        match tauri_plugin_clipboard_x::read_image(app.clone(), None).await {
            Ok(img) => {
                let path = img.path.to_string_lossy().into_owned();
                log::trace!(
                    "read clipboard image -> {path} ({}x{}, {}B)",
                    img.width,
                    img.height,
                    img.size
                );
                written_images.push(path.clone());
                entry.image = Some(ClipboardImage {
                    path,
                    size: img.size,
                    width: img.width,
                    height: img.height,
                    missing: false,
                });
            }
            Err(e) => log::warn!("read_image failed: {e}"),
        }
    }
    if tauri_plugin_clipboard_x::has_files().await.unwrap_or(false) {
        match tauri_plugin_clipboard_x::read_files().await {
            Ok(files) if !files.paths.is_empty() => {
                entry.files = Some(ClipboardFiles {
                    paths: files.paths,
                    size: files.size,
                });
            }
            _ => {}
        }
    }

    if entry.is_empty() {
        (None, written_images)
    } else {
        // 只记录元数据（含哪些格式），不记录剪贴板明文内容（隐私约束，见 core/log.rs）
        let kinds: Vec<&str> = [
            ("text", entry.text.is_some()),
            ("html", entry.html.is_some()),
            ("rtf", entry.rtf.is_some()),
            ("image", entry.image.is_some()),
            ("files", entry.files.is_some()),
        ]
        .iter()
        .filter(|(_, has)| *has)
        .map(|(kind, _)| *kind)
        .collect();
        log::trace!(
            "read clipboard: id={} kinds=[{}]",
            entry.id,
            kinds.join(",")
        );
        (Some(entry), written_images)
    }
}

/// 捕捉：监听事件触发。读剪贴板 → 落盘图片 → 去重置顶 → 即时淘汰 → 写回 store（契约 5.2）。
#[tauri::command]
pub async fn capture_clipboard(app: AppHandle) -> Result<Option<ClipboardEntry>, ApiError> {
    let (incoming, written_images) = read_clipboard(&app).await;
    let Some(incoming) = incoming else {
        log::trace!("capture_clipboard: no usable content, ignored");
        return Ok(None); // 空内容静默忽略（契约 5.2-2、D11）
    };

    // 串行化 store 段（读→去重置顶→写），防并发捕捉重复插入（见 CAPTURE_LOCK 注释）
    let (outcome, entries) = {
        let _guard = CAPTURE_LOCK.lock().unwrap();
        let store = StoreBackend::new(&app)?;
        let max = store.load_max_entries()?;
        let mut entries = store.load_entries()?;
        let outcome: InsertOutcome = dedup_promote_and_evict(&mut entries, incoming, max);
        store.save_entries(&entries)?;
        (outcome, entries)
    };
    if outcome.is_new {
        log::info!(
            "capture_clipboard: new entry id={} evicted={} total={}",
            outcome.entry.id,
            outcome.evicted_files.len(),
            entries.len()
        );
        // lan-sync 广播钩子（core::hooks）：新条目 → 广播（防环/开关/体积在 lan_sync 侧判断）
        let entry_json = serde_json::to_value(&outcome.entry).unwrap_or_default();
        crate::core::hooks::notify_new_entry(&app, &entry_json);
    } else {
        log::debug!(
            "capture_clipboard: dedup-promote entry id={} total={}",
            outcome.entry.id,
            entries.len()
        );
    }
    delete_files(&outcome.evicted_files);

    // 清理因去重置顶而丢弃的本次落盘图片：未被任何存活条目引用则立即删除，避免成为孤儿文件
    // （read_clipboard 已提前落盘，但去重命中旧条目时新图可能不被采纳）。
    for path in &written_images {
        let referenced = entries
            .iter()
            .any(|e| e.image.as_ref().is_some_and(|img| &img.path == path));
        if !referenced {
            match std::fs::remove_file(path) {
                Ok(()) => log::debug!("capture: removed dedup-discarded image {path}"),
                Err(e) => log::debug!("capture: failed to remove unreferenced image {path}: {e}"),
            }
        }
    }

    Ok(Some(outcome.entry))
}

/// 读取全部条目，按展示序返回（收藏区在前、区内按收藏时间倒序，其后按捕捉时间倒序），
/// 并计算图片缺失派生标记（契约 5.4-③、5.8）。
#[tauri::command]
pub async fn get_clipboard_history(app: AppHandle) -> Result<Vec<ClipboardEntry>, ApiError> {
    let store = StoreBackend::new(&app)?;
    let mut entries = store.load_entries()?;
    sort_for_display(&mut entries);
    for entry in &mut entries {
        if let Some(img) = &mut entry.image {
            img.missing = !Path::new(&img.path).exists();
        }
    }
    log::trace!("get_clipboard_history: {} entries", entries.len());
    Ok(entries)
}

/// 回写：按条目原始格式写回系统剪贴板（契约 5.5）。
#[tauri::command]
pub async fn write_clipboard_entry(app: AppHandle, id: String) -> Result<(), ApiError> {
    let store = StoreBackend::new(&app)?;
    let entries = store.load_entries()?;
    let entry = entries
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| entry_not_found(&id))?;

    let kind = if entry.html.is_some() {
        "html"
    } else if entry.rtf.is_some() {
        "rtf"
    } else if entry.text.is_some() {
        "text"
    } else if entry.image.is_some() {
        "image"
    } else {
        "files"
    };
    log::debug!("write_clipboard_entry: id={id} kind={kind}");

    if let Some(html) = &entry.html {
        tauri_plugin_clipboard_x::write_html(entry.text.clone().unwrap_or_default(), html.clone())
            .await
            .map_err(capture_err)?;
    } else if let Some(rtf) = &entry.rtf {
        tauri_plugin_clipboard_x::write_rtf(entry.text.clone().unwrap_or_default(), rtf.clone())
            .await
            .map_err(capture_err)?;
    } else if let Some(text) = &entry.text {
        tauri_plugin_clipboard_x::write_text(text.clone())
            .await
            .map_err(capture_err)?;
    } else if let Some(img) = &entry.image {
        if !img.missing {
            tauri_plugin_clipboard_x::write_image(img.path.clone())
                .await
                .map_err(capture_err)?;
        }
    } else if let Some(files) = &entry.files {
        tauri_plugin_clipboard_x::write_files(files.paths.clone())
            .await
            .map_err(capture_err)?;
    }
    Ok(())
}

/// 单条删除：删除条目并尝试删除其图片文件（失败留给定时兜底，契约 5.6）。
#[tauri::command]
pub async fn delete_clipboard_entry(app: AppHandle, id: String) -> Result<(), ApiError> {
    let store = StoreBackend::new(&app)?;
    let mut entries = store.load_entries()?;
    let pos = entries
        .iter()
        .position(|e| e.id == id)
        .ok_or_else(|| entry_not_found(&id))?;
    let removed = entries.remove(pos);
    if let Some(img) = &removed.image {
        let _ = std::fs::remove_file(&img.path);
    }
    log::debug!(
        "delete_clipboard_entry: id={id} had_image={}",
        removed.image.is_some()
    );
    store.save_entries(&entries)
}

/// 设置/取消收藏（契约 5.8）：幂等；重复收藏刷新收藏时间（收藏区重新置顶）。
/// 与 capture 同持 `CAPTURE_LOCK`，串行化「读 → 改 → 写」store 段，防丢失更新。
#[tauri::command]
pub async fn set_entry_favorite(
    app: AppHandle,
    id: String,
    favorited: bool,
) -> Result<(), ApiError> {
    let _guard = CAPTURE_LOCK.lock().unwrap();
    let store = StoreBackend::new(&app)?;
    let mut entries = store.load_entries()?;
    if !set_favorite(&mut entries, &id, favorited, &now_iso8601()) {
        return Err(entry_not_found(&id));
    }
    log::debug!("set_entry_favorite: id={id} favorited={favorited}");
    store.save_entries(&entries)
}

/// 清空全部：清空条目并删除图片目录下全部 .png（契约 5.6）。
#[tauri::command]
pub async fn clear_clipboard_history(app: AppHandle) -> Result<(), ApiError> {
    let store = StoreBackend::new(&app)?;
    store.save_entries(&[])?;
    if let Ok(dir) = image_dir(&app) {
        let removed = list_png_files(&dir);
        log::debug!(
            "clear_clipboard_history: removing {} image files",
            removed.len()
        );
        delete_files(&removed);
    }
    Ok(())
}

/// 定时兜底清理：扫描图片目录，删除无条目引用的孤儿图片（契约 5.4-②）。
#[tauri::command]
pub async fn cleanup_orphan_images(app: AppHandle) -> Result<CleanupResp, ApiError> {
    let store = StoreBackend::new(&app)?;
    let entries = store.load_entries()?;
    let dir = image_dir(&app)?;
    log::trace!("cleanup_orphan_images: scanning dir = {}", dir.display());
    let files = list_png_files(&dir);
    for f in &files {
        log::trace!("cleanup_orphan_images: scanned file = {}", f.display());
    }
    let referenced: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.image.as_ref())
        .map(|img| img.path.as_str())
        .collect();
    for r in &referenced {
        log::trace!("cleanup_orphan_images: referenced image = {r}");
    }
    let orphans = orphan_files(&entries, &files);
    for f in &orphans {
        log::trace!("cleanup_orphan_images: removing {}", f.display());
        let _ = std::fs::remove_file(f);
    }
    log::debug!(
        "cleanup_orphan_images: scanned={} referenced={} removed={}",
        files.len(),
        referenced.len(),
        orphans.len()
    );
    Ok(CleanupResp {
        removed: orphans.len() as u32,
    })
}

/// 读取当前上限 n。
#[tauri::command]
pub async fn get_max_entries(app: AppHandle) -> Result<u32, ApiError> {
    let store = StoreBackend::new(&app)?;
    Ok(store.load_max_entries()? as u32)
}

/// 设置上限 n（1~1024），超限立即截断（契约 5.7）。
#[tauri::command]
pub async fn set_max_entries(app: AppHandle, max_entries: u32) -> Result<SetMaxResp, ApiError> {
    let n = max_entries as usize;
    if !(1..=MAX_ENTRIES_LIMIT).contains(&n) {
        log::warn!("set_max_entries: invalid n={n} (range 1..={MAX_ENTRIES_LIMIT})");
        return Err(ApiError::new(
            ERR_INVALID_MAX,
            format!("max entries must be between 1 and {MAX_ENTRIES_LIMIT}, got {n}"),
        ));
    }

    let store = StoreBackend::new(&app)?;
    let mut entries = store.load_entries()?;
    let evicted = evict_over_limit(&mut entries, n);
    delete_files(&evicted);
    store.save_entries(&entries)?;
    store.save_max_entries(n)?;
    log::info!(
        "set_max_entries: n={n} evicted={} total={}",
        evicted.len(),
        entries.len()
    );
    Ok(SetMaxResp {
        max_entries: n as u32,
        evicted: evicted.len() as u32,
    })
}
