//! 节点层（core 横切）：局域网 libp2p 节点，供多功能共享。
//!
//! - `identity.rs`：ed25519 身份持久化（peerId 为终端稳定身份，不依赖 IP）
//! - `node.rs`：swarm 生命周期（mdns 发现 + gossipsub 多主题 pubsub），通用收发通道
//!
//! 消费方：`features/lan_sync`（剪贴板主题）、`features/lan_file`（公告主题）。
//! 0.3.0 起通道多主题化（契约 `docs/api/lan-file.md` §6.1）：
//! `Publish { topic, data }` / `PubsubMessage { topic, source, data }`——
//! 纯通道字段，业务语义仍留在各 feature；lan_sync 传固定主题，行为不变。

pub mod identity;
pub mod node;

pub use node::{NodeCommand, NodeConfig, NodeEvent, PeerNode};
