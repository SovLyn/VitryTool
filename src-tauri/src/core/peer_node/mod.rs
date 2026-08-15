//! 节点层（core 横切）：局域网 libp2p 节点，供多功能共享。
//!
//! - `identity.rs`：ed25519 身份持久化（peerId 为终端稳定身份，不依赖 IP）
//! - `node.rs`：swarm 生命周期（mdns 发现 + gossipsub pubsub），通用收发通道
//!
//! 首个消费方：`features/lan_sync`（契约 `docs/api/lan-sync.md`）。
//! 后续功能（文件传输、多设备会话）直接复用本层，见决策 F6。

pub mod identity;
pub mod node;

pub use node::{NodeCommand, NodeConfig, NodeEvent, PeerNode};
