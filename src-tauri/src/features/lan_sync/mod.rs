//! 局域网剪贴板同步功能域（lan-sync，0.2.5）。
//!
//! - `commands.rs`：`#[tauri::command]` 薄壳（8 命令 + 事件）
//! - `service.rs`：纯逻辑（设置/信封/指纹/收件箱分桶）
//! - `store.rs`：持久化（lan-inbox.json / lan-sync.json）
//! - `state.rs`：运行时共享态 + 消费者线程 + 广播钩子
//! - `tests.rs`：开发者测试（dt）
//!
//! 契约：`docs/api/lan-sync.md`；节点层：`core/peer_node`；
//! 决策记录：`dev/interface-drafts/lan-sync-contract-draft.md`。

pub mod commands;
pub mod service;
pub mod state;
mod store;

#[cfg(test)]
mod tests;

pub use commands::*;

use tauri::Manager;

/// 启动 lan-sync：加载/创建身份 → 启动节点（core/peer_node）→ 初始化业务状态（setup 调用）。
pub fn init_node(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    let (keypair, created) = crate::core::peer_node::identity::load_or_create(&data_dir.join("peer-key.json"));
    let self_peer_id = keypair.public().to_peer_id().to_base58();

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let node = crate::core::peer_node::PeerNode::spawn(crate::core::peer_node::NodeConfig {
        keypair,
        topic: service::TOPIC.to_string(),
        event_tx,
    })?;

    *app.state::<crate::core::state::AppState>().peer_node.lock().unwrap() = Some(node);
    state::init(app.handle(), event_rx, self_peer_id)?;
    let _ = created;
    Ok(())
}
