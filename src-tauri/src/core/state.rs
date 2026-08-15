//! 全局状态。

use crate::core::peer_node::PeerNode;
use std::sync::Mutex;

/// 应用全局状态。
///
/// 所有功能共享的状态挂载在这里；功能私有的状态放在各自 mod 内
/// （如 lan-sync 的收件箱/设置共享态在 `features/lan_sync/state.rs`）。
#[derive(Default)]
pub struct AppState {
    /// 局域网 libp2p 节点（core/peer_node），setup 时启动、退出时停止。
    pub peer_node: Mutex<Option<PeerNode>>,
}

impl AppState {
    /// 创建全局状态实例。
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_constructible() {
        let state = AppState::new();
        assert!(state.peer_node.lock().unwrap().is_none());
    }
}
