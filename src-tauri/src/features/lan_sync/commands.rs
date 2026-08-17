//! lan-sync 命令（薄壳：参数解析 + 状态获取/修改，业务逻辑在 service / state）。
//!
//! 契约：`docs/api/lan-sync.md` 第 2、4 节。

use super::service::{
    inbox_for_display, validate_terminal_name, InboxData, LanSyncStatus, TERMINAL_NAME_MAX_LEN,
};
use super::state::{shared, INBOX_UPDATED_EVENT, SETTINGS_UPDATED_EVENT};
use super::store::{InboxStore, SettingsStore, StoreBackend};
use crate::core::error::ApiError;
use crate::core::state::AppState;
use tauri::{AppHandle, Emitter, Manager};

const ERR_STORAGE: &str = "lan.storage_error";
const ERR_NOT_FOUND: &str = "lan.entry_not_found";
const ERR_INVALID_NAME: &str = "lan.invalid_name";
const ERR_NODE: &str = "lan.peer_node_error";

fn storage_err(err: impl std::fmt::Display) -> ApiError {
    log::error!("{ERR_STORAGE}: {err}");
    ApiError::new(ERR_STORAGE, format!("lan-sync store error: {err}"))
}

fn node_err(err: impl std::fmt::Display) -> ApiError {
    log::error!("{ERR_NODE}: {err}");
    ApiError::new(ERR_NODE, format!("lan-sync node error: {err}"))
}

fn entry_not_found(id: &str) -> ApiError {
    log::error!("{ERR_NOT_FOUND}: {id}");
    ApiError::new(ERR_NOT_FOUND, format!("inbox entry not found: {id}"))
}

fn shared_or_err() -> Result<std::sync::Arc<std::sync::Mutex<super::state::LanSyncShared>>, ApiError>
{
    shared()
        .cloned()
        .ok_or_else(|| node_err("lan-sync not initialized"))
}

/// 节点状态（契约 2：getLanSyncStatus）。
#[tauri::command]
pub async fn get_lan_sync_status(app: AppHandle) -> Result<LanSyncStatus, ApiError> {
    let shared = shared_or_err()?;
    let (peer_id, terminal_name, broadcast_enabled, receive_enabled, node_running) = {
        let g = shared.lock().unwrap();
        (
            g.self_peer_id.clone(),
            g.settings.terminal_name.clone(),
            g.settings.broadcast_enabled,
            g.settings.receive_enabled,
            g.node_running,
        )
    };
    let peer_count = app
        .state::<AppState>()
        .peer_node
        .lock()
        .unwrap()
        .as_ref()
        .map(|n| n.peer_count())
        .unwrap_or(0);
    Ok(LanSyncStatus {
        peer_id,
        terminal_name,
        broadcast_enabled,
        receive_enabled,
        node_running,
        peer_count,
    })
}

/// 开/关广播（契约 5.7）。
#[tauri::command]
pub async fn set_lan_sync_broadcast(app: AppHandle, enabled: bool) -> Result<(), ApiError> {
    let shared = shared_or_err()?;
    let settings = {
        let mut g = shared.lock().unwrap();
        g.settings.broadcast_enabled = enabled;
        g.settings.clone()
    };
    let backend = StoreBackend::new(&app).map_err(storage_err)?;
    backend.save_settings(&settings).map_err(storage_err)?;
    let _ = app.emit(
        SETTINGS_UPDATED_EVENT,
        serde_json::json!({ "broadcast": enabled }),
    );
    log::info!("lan_sync: broadcast={enabled}");
    Ok(())
}

/// 开/关接收（契约 5.7）。
#[tauri::command]
pub async fn set_lan_sync_receive(app: AppHandle, enabled: bool) -> Result<(), ApiError> {
    let shared = shared_or_err()?;
    let settings = {
        let mut g = shared.lock().unwrap();
        g.settings.receive_enabled = enabled;
        g.settings.clone()
    };
    let backend = StoreBackend::new(&app).map_err(storage_err)?;
    backend.save_settings(&settings).map_err(storage_err)?;
    let _ = app.emit(
        SETTINGS_UPDATED_EVENT,
        serde_json::json!({ "receive": enabled }),
    );
    log::info!("lan_sync: receive={enabled}");
    Ok(())
}

/// 设置终端名（契约 5.7）。
#[tauri::command]
pub async fn set_lan_sync_terminal_name(app: AppHandle, name: String) -> Result<(), ApiError> {
    let trimmed = name.trim().to_string();
    if !validate_terminal_name(&trimmed) {
        log::warn!(
            "set_lan_sync_terminal_name: invalid name (len≤{TERMINAL_NAME_MAX_LEN}, non-empty)"
        );
        return Err(ApiError::new(
            ERR_INVALID_NAME,
            format!("terminal name must be non-empty and at most {TERMINAL_NAME_MAX_LEN} chars"),
        ));
    }
    let shared = shared_or_err()?;
    let settings = {
        let mut g = shared.lock().unwrap();
        g.settings.terminal_name = trimmed.clone();
        g.settings.clone()
    };
    let backend = StoreBackend::new(&app).map_err(storage_err)?;
    backend.save_settings(&settings).map_err(storage_err)?;
    log::info!("lan_sync: terminal name = {trimmed}");
    Ok(())
}

/// 收件箱全量（契约 2：getLanInbox；展示序：节点按最新条目倒序、桶内按接收时间倒序）。
#[tauri::command]
pub async fn get_lan_inbox() -> Result<InboxData, ApiError> {
    let shared = shared_or_err()?;
    let inbox = {
        let g = shared.lock().unwrap();
        inbox_for_display(&g.inbox)
    };
    log::trace!(
        "get_lan_inbox: {} nodes",
        inbox.nodes.iter().map(|n| n.entries.len()).sum::<usize>()
    );
    Ok(inbox)
}

/// 回写：按原格式写系统剪贴板 → 本机 capture 进历史（防环不重广播，契约 5.5）。
/// 移动端（契约 mobile 5.3）：写纯文本（5.2 提取）→ 经 hooks 显式入本地历史，**不广播**。
#[tauri::command]
pub async fn write_lan_inbox_entry(_app: AppHandle, id: String) -> Result<(), ApiError> {
    let shared = shared_or_err()?;
    let entry = {
        let g = shared.lock().unwrap();
        g.inbox
            .nodes
            .iter()
            .flat_map(|n| n.entries.iter())
            .find(|e| e.id == id)
            .cloned()
    }
    .ok_or_else(|| entry_not_found(&id))?;

    #[cfg(desktop)]
    {
        let kind = if entry.html.is_some() {
            "html"
        } else if entry.rtf.is_some() {
            "rtf"
        } else if entry.text.is_some() {
            "text"
        } else if entry.file_paths.is_some() {
            "files"
        } else {
            "image"
        };
        log::debug!("write_lan_inbox_entry: id={id} kind={kind}");

        if let Some(html) = &entry.html {
            tauri_plugin_clipboard_x::write_html(
                entry.text.clone().unwrap_or_default(),
                html.clone(),
            )
            .await
            .map_err(|e| ApiError::new("lan.peer_node_error", format!("write clipboard: {e}")))?;
        } else if let Some(rtf) = &entry.rtf {
            tauri_plugin_clipboard_x::write_rtf(
                entry.text.clone().unwrap_or_default(),
                rtf.clone(),
            )
            .await
            .map_err(|e| ApiError::new("lan.peer_node_error", format!("write clipboard: {e}")))?;
        } else if let Some(text) = &entry.text {
            tauri_plugin_clipboard_x::write_text(text.clone())
                .await
                .map_err(|e| {
                    ApiError::new("lan.peer_node_error", format!("write clipboard: {e}"))
                })?;
        } else if let Some(paths) = &entry.file_paths {
            tauri_plugin_clipboard_x::write_files(paths.clone())
                .await
                .map_err(|e| {
                    ApiError::new("lan.peer_node_error", format!("write clipboard: {e}"))
                })?;
        } else if let Some(meta) = &entry.image_meta {
            // 首版图片仅元数据：写占位文本（契约 5.5）
            let dims = match (meta.width, meta.height) {
                (Some(w), Some(h)) => format!(" ({w}x{h})"),
                _ => String::new(),
            };
            tauri_plugin_clipboard_x::write_text(format!("[图片] {}{dims}", meta.name))
                .await
                .map_err(|e| {
                    ApiError::new("lan.peer_node_error", format!("write clipboard: {e}"))
                })?;
        }
        Ok(())
    }

    #[cfg(mobile)]
    {
        let placeholder = entry.image_meta.as_ref().map(|meta| {
            let dims = match (meta.width, meta.height) {
                (Some(w), Some(h)) => format!(" ({w}x{h})"),
                _ => String::new(),
            };
            format!("[图片] {}{dims}", meta.name)
        });
        let Some(plain) = crate::core::platform::mobile_writable_text(
            entry.text.as_deref(),
            entry.html.as_deref(),
            placeholder,
        ) else {
            log::warn!("write_lan_inbox_entry: id={id} unsupported on mobile (files only)");
            return Err(ApiError::new(
                "clipboard.write_unsupported",
                "inbox entry contains only file paths, cannot write on mobile",
            ));
        };
        // 写剪贴板 + 显式入历史（hooks 由 clipboard_history 在 setup 注册）
        crate::core::hooks::mobile_clipboard_write(&_app, &plain)
            .map_err(|e| ApiError::new("lan.peer_node_error", format!("write clipboard: {e}")))
    }
}

/// 单条删除（契约 5.3）。
#[tauri::command]
pub async fn delete_lan_inbox_entry(app: AppHandle, id: String) -> Result<(), ApiError> {
    let shared = shared_or_err()?;
    let inbox = {
        let mut g = shared.lock().unwrap();
        if !super::service::delete_entry(&mut g.inbox, &id) {
            return Err(entry_not_found(&id));
        }
        g.inbox.clone()
    };
    let backend = StoreBackend::new(&app).map_err(storage_err)?;
    backend.save_inbox(&inbox).map_err(storage_err)?;
    let _ = app.emit(
        INBOX_UPDATED_EVENT,
        serde_json::json!({ "reason": "deleted", "id": id }),
    );
    log::info!("lan_sync: inbox entry deleted");
    Ok(())
}

/// 清空收件箱。
#[tauri::command]
pub async fn clear_lan_inbox(app: AppHandle) -> Result<(), ApiError> {
    let shared = shared_or_err()?;
    let inbox = {
        let mut g = shared.lock().unwrap();
        super::service::clear_inbox(&mut g.inbox);
        g.inbox.clone()
    };
    let backend = StoreBackend::new(&app).map_err(storage_err)?;
    backend.save_inbox(&inbox).map_err(storage_err)?;
    let _ = app.emit(
        INBOX_UPDATED_EVENT,
        serde_json::json!({ "reason": "cleared" }),
    );
    log::info!("lan_sync: inbox cleared");
    Ok(())
}
