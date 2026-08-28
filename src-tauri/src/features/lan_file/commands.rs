//! lan-file 命令薄壳（契约 `docs/api/lan-file.md` 第 2 节，7 命令）。
//!
//! 命令注册：桌面注册全部 7 命令；移动端不注册任何 lan-file 命令（契约 5.9，
//! 见 lib.rs 的 `generate_handler!` 拆分）。业务在 `state.rs` / `service.rs`。
//!
//! 注：`#[tauri::command]` 宏生成的包装未被引用时误报 never used，模块级抑制。

#![allow(dead_code)]

use super::service::{LanFilePeersResp, LanFileStatus, SendLanFileResp};
use super::state;
use crate::core::error::ApiError;
use tauri::AppHandle;

const ERR_NODE: &str = "lan_file.node_not_running";
const ERR_STORAGE: &str = "lan_file.storage_error";
const ERR_INVALID: &str = "lan_file.invalid_path";
const ERR_NOT_FOUND: &str = "lan_file.peer_not_found";

fn not_initialized() -> ApiError {
    log::error!("{ERR_NODE}: lan-file not initialized");
    ApiError::new(ERR_NODE, "lan-file not initialized")
}

fn storage_err(e: impl std::fmt::Display) -> ApiError {
    log::error!("{ERR_STORAGE}: {e}");
    ApiError::new(ERR_STORAGE, format!("lan-file store error: {e}"))
}

/// 状态（契约 2：getLanFileStatus）。
#[tauri::command]
pub async fn get_lan_file_status() -> Result<LanFileStatus, ApiError> {
    Ok(state::status_snapshot())
}

/// 可传终端列表（契约 2：getLanFilePeers）。
#[tauri::command]
pub async fn get_lan_file_peers(app: AppHandle) -> Result<LanFilePeersResp, ApiError> {
    let peers = state::peers_snapshot(&app);
    Ok(LanFilePeersResp { peers })
}

/// 发起传输任务（契约 2：sendLanFile；校验失败直接 Err，任务不创建）。
#[tauri::command]
pub async fn send_lan_file(
    app: AppHandle,
    peer_id: String,
    file_paths: Vec<String>,
) -> Result<SendLanFileResp, ApiError> {
    let shared = state::shared().cloned().ok_or_else(not_initialized)?;
    // 开关检查
    if !shared.lock().unwrap().settings.enabled {
        return Err(ApiError::new(
            super::service::err::NOT_ENABLED,
            "file sharing disabled",
        ));
    }
    // 前置校验（数量/路径长度在 validate 内；此处先挡空列表快速失败）
    if file_paths.is_empty() {
        return Err(ApiError::new(ERR_INVALID, "empty file list"));
    }
    // 忙检查（单会话槽前置判断，真实占用在任务内完成）
    {
        let g = shared.lock().unwrap();
        if g.active_transfer.is_some() {
            return Err(ApiError::new(
                super::service::err::BUSY,
                "a transfer is active",
            ));
        }
        // 对端在线 + 支持交互传输
        let Some(entry) = g.announces.iter().find(|e| e.announce.peer_id == peer_id) else {
            return Err(ApiError::new(
                ERR_NOT_FOUND,
                format!("peer {peer_id} not announced"),
            ));
        };
        if !entry.announce.supports_interactive() {
            return Err(ApiError::new(
                super::service::err::PEER_UNSUPPORTED,
                "peer does not support interactive transfer",
            ));
        }
    }

    let transfer_id = uuid::Uuid::new_v4().to_string();
    let (signing, self_peer_id) = task_ctx(&app)?;
    // 真机路径：学习对端地址——由公告消费者记录（当前实现依赖入站连接学习 +
    // 单局域网网段直连）；发起方 dial 用 `peer_ip(host):tcp_port`。
    let peer_addr = state::peer_addr(&peer_id);

    // 任务体跑在独立 tokio 运行时线程（命令立即返回 transferId，进度走事件）
    let app2 = app.clone();
    let tid = transfer_id.clone();
    std::thread::Builder::new()
        .name(format!("lan-file-send-{tid}"))
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("lan_file: send task runtime failed: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                if let Some(addr) = peer_addr {
                    state::learn_peer_addr(&peer_id, &addr);
                }
                let _ =
                    state::send_task(app2, signing, self_peer_id, peer_id, file_paths, tid).await;
            });
        })
        .map_err(|e| ApiError::new(ERR_NODE, format!("spawn send task: {e}")))?;

    Ok(SendLanFileResp { transfer_id })
}

/// 任务上下文：签名密钥 + 本机 peerId（签名密钥由 lan_file::init 存入共享态）。
fn task_ctx(app: &AppHandle) -> Result<(ed25519_dalek::SigningKey, String), ApiError> {
    state::task_context(app).ok_or_else(not_initialized)
}

/// 接受提议（契约 2：acceptLanFile；未知终端同时写信任表 = TOFU 确认）。
#[tauri::command]
pub async fn accept_lan_file(app: AppHandle, transfer_id: String) -> Result<(), ApiError> {
    let (peer_id, terminal_name) = state::pending_offer_peer(&transfer_id)
        .ok_or_else(|| ApiError::new(ERR_NOT_FOUND, format!("offer {transfer_id} not found")))?;
    // TOFU：accept 即信任（契约 5.4；已有条目则刷新）
    state::trust_peer(&app, &peer_id, &terminal_name).map_err(storage_err)?;
    state::resolve_offer(&transfer_id, state::TaskCommand::Accept);
    log::info!("lan_file: offer {transfer_id} accepted (peer={peer_id})");
    Ok(())
}

/// 拒绝提议（契约 2：rejectLanFile；通知对端 + 清理，无状态残留）。
#[tauri::command]
pub async fn reject_lan_file(_app: AppHandle, transfer_id: String) -> Result<(), ApiError> {
    state::resolve_offer(&transfer_id, state::TaskCommand::Reject);
    log::info!("lan_file: offer {transfer_id} rejected");
    Ok(())
}

/// 显式取消（契约 2：cancelLanFileTransfer = 永久终止：墓碑 + 删 tmp + 通知对端）。
#[tauri::command]
pub async fn cancel_lan_file_transfer(
    _app: AppHandle,
    transfer_id: String,
) -> Result<(), ApiError> {
    state::cancel_transfer(&transfer_id);
    log::info!("lan_file: transfer {transfer_id} cancelled by user");
    Ok(())
}

/// 总开关（契约 2：setLanFileEnabled；关闭 = 停数据面 + 终止活跃任务 + 清图片队列）。
#[tauri::command]
pub async fn set_lan_file_enabled(app: AppHandle, enabled: bool) -> Result<(), ApiError> {
    state::set_enabled_impl(&app, enabled).map_err(storage_err)
}

/// 设置页「已信任终端」移除（契约 5.4）。
#[tauri::command]
pub async fn remove_lan_file_trusted_peer(app: AppHandle, peer_id: String) -> Result<(), ApiError> {
    state::untrust_peer(&app, &peer_id).map_err(storage_err)?;
    log::info!("lan_file: trusted peer removed: {peer_id}");
    Ok(())
}

/// 已信任终端列表（设置页展示）。
#[tauri::command]
pub async fn get_lan_file_trusted_peers() -> Result<Vec<super::store::TrustedPeer>, ApiError> {
    let shared = state::shared().cloned().ok_or_else(not_initialized)?;
    let g = shared.lock().unwrap();
    Ok(g.settings.trusted_peers.clone())
}

/// 供 lib.rs 的 lan_sync 消费者转发公告主题消息。
pub fn forward_announce_from_node(source: &str, data: &[u8]) {
    state::forward_announce(source, data);
}

/// 供 capture 钩子调用（图片通道排队）。
pub fn on_new_entry(app: &AppHandle, entry: &serde_json::Value) {
    state::queue_image_offers(app, entry);
}
