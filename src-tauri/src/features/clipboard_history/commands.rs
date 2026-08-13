//! 剪贴板历史命令（薄壳：参数解析 + 状态获取，业务逻辑在 service / store）。
//!
//! 命令均为异步（内部等待插件异步 API）。所有业务动作由前端发起，
//! 后端无自有时钟（见 `docs/adr/0001-clipboard-capture-via-webview-events.md`）。

use super::service::{
    dedup_promote_and_evict, evict_over_limit, orphan_files, CleanupResp, ClipboardEntry,
    ClipboardFiles, ClipboardImage, InsertOutcome, SetMaxResp, MAX_ENTRIES_LIMIT,
};
use super::store::{HistoryStore, StoreBackend};
use crate::core::error::ApiError;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// 插件保存图片的目录（插件默认路径，相对应用数据目录，见契约 5.4）。
const IMAGE_DIR_REL: &str = "tauri-plugin-clipboard-x/images";

const ERR_CAPTURE: &str = "clipboard.capture_failed";
const ERR_ENTRY_NOT_FOUND: &str = "clipboard.entry_not_found";
const ERR_INVALID_MAX: &str = "clipboard.invalid_max_entries";

fn capture_err(err: impl std::fmt::Display) -> ApiError {
    ApiError::new(ERR_CAPTURE, format!("read clipboard failed: {err}"))
}

fn entry_not_found(id: &str) -> ApiError {
    ApiError::new(ERR_ENTRY_NOT_FOUND, format!("entry not found: {id}"))
}

fn image_dir(app: &AppHandle) -> Result<PathBuf, ApiError> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| ApiError::new("clipboard.storage_error", e.to_string()))?;
    Ok(base.join(IMAGE_DIR_REL))
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

/// 读取剪贴板各格式（逐格式独立容错，契约 5.2-1），无可用内容返回 `None`。
async fn read_clipboard(app: &AppHandle) -> Result<Option<ClipboardEntry>, ApiError> {
    let mut entry = ClipboardEntry {
        id: uuid::Uuid::new_v4().to_string(),
        captured_at: now_iso8601(),
        text: None,
        html: None,
        rtf: None,
        image: None,
        files: None,
    };

    if tauri_plugin_clipboard_x::has_text().await.unwrap_or(false) {
        match tauri_plugin_clipboard_x::read_text().await {
            Ok(text) => entry.text = Some(text),
            Err(e) => eprintln!("[clipboard_history] read_text failed: {e}"),
        }
    }
    if tauri_plugin_clipboard_x::has_html().await.unwrap_or(false) {
        match tauri_plugin_clipboard_x::read_html().await {
            Ok(html) => entry.html = Some(html),
            Err(e) => eprintln!("[clipboard_history] read_html failed: {e}"),
        }
    }
    if tauri_plugin_clipboard_x::has_rtf().await.unwrap_or(false) {
        match tauri_plugin_clipboard_x::read_rtf().await {
            Ok(rtf) => entry.rtf = Some(rtf),
            Err(e) => eprintln!("[clipboard_history] read_rtf failed: {e}"),
        }
    }
    if tauri_plugin_clipboard_x::has_image().await.unwrap_or(false) {
        match tauri_plugin_clipboard_x::read_image(app.clone(), None).await {
            Ok(img) => {
                entry.image = Some(ClipboardImage {
                    path: img.path.to_string_lossy().into_owned(),
                    size: img.size,
                    width: img.width,
                    height: img.height,
                    missing: false,
                });
            }
            Err(e) => eprintln!("[clipboard_history] read_image failed: {e}"),
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
        Ok(None)
    } else {
        Ok(Some(entry))
    }
}

/// 捕捉：监听事件触发。读剪贴板 → 落盘图片 → 去重置顶 → 即时淘汰 → 写回 store（契约 5.2）。
#[tauri::command]
pub async fn capture_clipboard(app: AppHandle) -> Result<Option<ClipboardEntry>, ApiError> {
    let Some(incoming) = read_clipboard(&app).await? else {
        return Ok(None); // 空内容静默忽略（契约 5.2-2、D11）
    };

    let store = StoreBackend::new(&app)?;
    let max = store.load_max_entries()?;
    let mut entries = store.load_entries()?;
    let outcome: InsertOutcome = dedup_promote_and_evict(&mut entries, incoming, max);
    delete_files(&outcome.evicted_files);
    store.save_entries(&entries)?;
    Ok(Some(outcome.entry))
}

/// 读取全部条目（最新在前），并计算图片缺失派生标记（契约 5.4-③）。
#[tauri::command]
pub async fn get_clipboard_history(app: AppHandle) -> Result<Vec<ClipboardEntry>, ApiError> {
    let store = StoreBackend::new(&app)?;
    let mut entries = store.load_entries()?;
    for entry in &mut entries {
        if let Some(img) = &mut entry.image {
            img.missing = !Path::new(&img.path).exists();
        }
    }
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
    store.save_entries(&entries)
}

/// 清空全部：清空条目并删除图片目录下全部 .png（契约 5.6）。
#[tauri::command]
pub async fn clear_clipboard_history(app: AppHandle) -> Result<(), ApiError> {
    let store = StoreBackend::new(&app)?;
    store.save_entries(&[])?;
    if let Ok(dir) = image_dir(&app) {
        delete_files(&list_png_files(&dir));
    }
    Ok(())
}

/// 定时兜底清理：扫描图片目录，删除无条目引用的孤儿图片（契约 5.4-②）。
#[tauri::command]
pub async fn cleanup_orphan_images(app: AppHandle) -> Result<CleanupResp, ApiError> {
    let store = StoreBackend::new(&app)?;
    let entries = store.load_entries()?;
    let dir = image_dir(&app)?;
    let files = list_png_files(&dir);
    let orphans = orphan_files(&entries, &files);
    delete_files(&orphans);
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
    Ok(SetMaxResp {
        max_entries: n as u32,
        evicted: evicted.len() as u32,
    })
}
