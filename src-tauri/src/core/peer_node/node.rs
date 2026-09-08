//! libp2p 节点（core 横切层）：身份 + swarm（mdns 发现 + gossipsub pubsub）+ 生命周期。
//!
//! 设计（契约 `docs/api/lan-sync.md` 5.1；可行性调研 `dev/interface-drafts/lan-sync-research.md`）：
//! - 独立 tokio runtime 后台线程运行 swarm，与应用生命周期同进退（setup 启动、退出时停止）；
//! - 通用 pubsub 通道：命令 `Publish { topic, data }` 进、事件 `PubsubMessage { topic, source, data }` 出，
//!   **不携带任何业务语义**（0.3.0 起多主题：lan_sync 用剪贴板主题、lan_file 用公告主题，
//!   业务语义在各自 feature，契约 `docs/api/lan-file.md` 6.1）；
//!
//! 实测关键点（已复验）：
//! - 必须显式 `listen_on`，否则 mDNS 公告无 TXT dnsaddr 记录，对端解析不出地址；
//! - 发布前对端需完成 gossipsub 订阅握手（否则 NoPeersSubscribedToTopic）；
//! - 0.56 builder：`with_tcp`/`with_behaviour` 返回 Result 需 `?`，`with_quic` 不返回；
//! - mdns 0.48：`Behaviour::new(Config, peer_id)` 只收两个参数。

use futures::StreamExt;
use libp2p::{
    gossipsub::{self, MessageAuthenticity, ValidationMode},
    identity::Keypair,
    mdns,
    swarm::{NetworkBehaviour, SwarmEvent},
    Multiaddr,
};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

/// 节点配置。
pub struct NodeConfig {
    /// 节点身份（持久化，见 identity.rs）。
    pub keypair: Keypair,
    /// 订阅并发布的 gossipsub 主题列表（0.3.0 多主题：剪贴板 + 公告，契约 lan-file 6.1）。
    pub topics: Vec<String>,
    /// 节点事件通道（消费者为各 feature 业务线程）。
    pub event_tx: Sender<NodeEvent>,
}

/// 节点命令（业务线程 → 节点线程）。
#[derive(Debug)]
pub enum NodeCommand {
    /// 向指定主题发布原始字节（已由业务方序列化；主题未订阅时忽略并记日志）。
    Publish { topic: String, data: Vec<u8> },
    /// 查询当前已连接 peer 数。
    PeerCount(Sender<usize>),
    /// 停止节点（线程退出）。
    Shutdown,
}

/// 节点事件（节点线程 → 业务线程）。
#[derive(Debug)]
pub enum NodeEvent {
    /// 收到主题消息（source 为发送方 peerId，data 为原始字节；业务方按 topic 分发）。
    PubsubMessage {
        topic: String,
        source: String,
        data: Vec<u8>,
    },
    /// 已连接 peer 数变化（用于状态展示）。
    PeerCountChanged(usize),
    /// 与新 peer 建立连接（0.3.0：lan_file 公告桥据此立即补发公告——
    /// 启动时公告因「无订阅对端」被丢弃，若只靠 5min 周期重发，
    /// 双方互等对方先发会造成最长 5 分钟的「发现盲区」，契约 lan-file 5.2
    /// 「mDNS 发现新对端后补公告」的落地钩子）。
    /// `addr`：对端在 mdns 上通告的地址（如 `/ip4/192.168.31.203/tcp/12345`），
    /// lan_file 解析出 IP 后用于 VLF 数据面 dial（公告本身不带 IP）。
    PeerConnected { peer_id: String, addr: String },
}

/// 运行中的节点句柄（业务侧持有）。
pub struct PeerNode {
    command_tx: Sender<NodeCommand>,
    handle: Option<JoinHandle<()>>,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
}

impl Behaviour {
    fn new(
        keypair: &Keypair,
        topics: &[String],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let peer_id = keypair.public().to_peer_id();
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .validation_mode(ValidationMode::Permissive) // 原型沿用；生产可换 Strict + report
            .build()?;
        let mut gossipsub = gossipsub::Behaviour::new(
            MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
        )?;
        for topic in topics {
            let topic = gossipsub::IdentTopic::new(topic.clone());
            gossipsub.subscribe(&topic)?;
        }
        let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id)?;
        Ok(Self { gossipsub, mdns })
    }
}

impl PeerNode {
    /// 启动节点：独立线程 + tokio runtime 运行 swarm。
    pub fn spawn(config: NodeConfig) -> Result<Self, String> {
        let (command_tx, command_rx) = std::sync::mpsc::channel::<NodeCommand>();
        let local_peer_id = config.keypair.public().to_peer_id();
        let topics = config.topics.join(", ");
        let handle = std::thread::Builder::new()
            .name("lan-peer-node".into())
            .spawn(move || run_swarm(config, command_rx))
            .map_err(|e| format!("spawn node thread failed: {e}"))?;
        log::info!("peer_node: spawned, peer_id={local_peer_id} topics=[{topics}]");
        Ok(Self {
            command_tx,
            handle: Some(handle),
        })
    }

    /// 向指定主题发布一条 pubsub 消息（非阻塞；节点线程处理）。
    pub fn publish(&self, topic: &str, data: Vec<u8>) {
        let _ = self.command_tx.send(NodeCommand::Publish {
            topic: topic.to_string(),
            data,
        });
    }

    /// 查询当前已连接 peer 数（带 200ms 超时的同步查询；通道故障返回 0）。
    pub fn peer_count(&self) -> usize {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.command_tx.send(NodeCommand::PeerCount(tx)).is_err() {
            return 0;
        }
        rx.recv_timeout(std::time::Duration::from_millis(200))
            .unwrap_or_default()
    }

    /// 停止节点并等待线程退出。
    pub fn shutdown(&mut self) {
        let _ = self.command_tx.send(NodeCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        log::info!("peer_node: shut down");
    }
}

/// 节点线程主循环：tokio runtime 内运行 swarm。
fn run_swarm(config: NodeConfig, command_rx: Receiver<NodeCommand>) {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("lan-swarm")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("peer_node: tokio runtime build failed: {e}");
            return;
        }
    };
    let _ = rt.block_on(async_main(config, command_rx));
}

async fn async_main(config: NodeConfig, command_rx: Receiver<NodeCommand>) -> Result<(), String> {
    let topics = config.topics.clone();
    let keypair = config.keypair;
    let event_tx = config.event_tx;
    let local_peer_id = keypair.public().to_peer_id();

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            (libp2p::tls::Config::new, libp2p::noise::Config::new),
            libp2p::yamux::Config::default,
        )
        .map_err(|e| format!("tcp transport: {e}"))?
        .with_quic()
        .with_behaviour(|key| Behaviour::new(key, &topics))
        .map_err(|e| format!("behaviour: {e}"))?
        .build();

    swarm
        .listen_on(
            "/ip4/0.0.0.0/tcp/0"
                .parse()
                .map_err(|e| format!("addr: {e}"))?,
        )
        .map_err(|e| format!("listen tcp: {e}"))?;
    swarm
        .listen_on(
            "/ip4/0.0.0.0/udp/0/quic-v1"
                .parse()
                .map_err(|e| format!("addr: {e}"))?,
        )
        .map_err(|e| format!("listen quic: {e}"))?;

    log::info!("peer_node: listening, peer_id={local_peer_id}");
    let subscribed: HashSet<gossipsub::TopicHash> = topics
        .iter()
        .map(|t| gossipsub::TopicHash::from_raw(t.clone()))
        .collect();
    // mdns 发现的对端地址表（peerId → 通告地址；供 PeerConnected 事件带出，lan_file 数据面 dial 用）
    let mut mdns_addrs: HashMap<libp2p::PeerId, Multiaddr> = HashMap::new();
    let mut peer_count: usize = 0;
    let mut last_peer_count: Option<usize> = None;

    loop {
        // 命令通道：poll 非阻塞，兼顾 swarm 事件
        match command_rx.try_recv() {
            Ok(NodeCommand::Publish { topic, data }) => {
                let topic_hash = gossipsub::TopicHash::from_raw(topic);
                if !subscribed.contains(&topic_hash) {
                    log::warn!("peer_node: publish to unsubscribed topic {topic_hash}, dropped");
                    continue;
                }
                match swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic_hash.clone(), data)
                {
                    Ok(_) => {}
                    Err(e) => log::debug!("peer_node: publish failed: {e}"),
                }
            }
            Ok(NodeCommand::PeerCount(reply)) => {
                let _ = reply.send(peer_count);
            }
            Ok(NodeCommand::Shutdown) => {
                log::info!("peer_node: shutdown requested");
                break;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => break,
        }

        let event = tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => continue,
            ev = swarm.select_next_some() => ev,
        };

        match event {
            SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
                for (peer_id, addr) in peers {
                    if peer_id != *swarm.local_peer_id() {
                        log::debug!("peer_node: mdns discovered {peer_id} at {addr}");
                        // 记录通告地址（lan_file 数据面 dial 用 IP）
                        mdns_addrs.insert(peer_id, addr.clone());
                        let _ = swarm.dial(addr);
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
                for (peer_id, _) in peers {
                    log::debug!("peer_node: mdns expired {peer_id}");
                    mdns_addrs.remove(&peer_id);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message,
                ..
            })) => {
                let _ = event_tx.send(NodeEvent::PubsubMessage {
                    topic: message.topic.as_str().to_string(),
                    source: propagation_source.to_base58(),
                    data: message.data,
                });
            }
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                peer_count += 1;
                log::info!("peer_node: connected to {peer_id} (count={peer_count})");
                // 对端地址：优先取连接实际端点（入站/出站都可靠——出站是 dial 地址，
                // 入站是 send_back_addr；mdns_addrs 仅作兜底，入站连接时本机可能
                // 尚未完成自己的 mDNS 查询而表内无记录）。
                let addr = match &endpoint {
                    libp2p::core::ConnectedPoint::Dialer { address, .. } => address.clone(),
                    libp2p::core::ConnectedPoint::Listener { send_back_addr, .. } => {
                        send_back_addr.clone()
                    }
                };
                let addr_str = if !addr.as_ref().is_empty() {
                    addr.to_string()
                } else {
                    mdns_addrs
                        .get(&peer_id)
                        .cloned()
                        .map(|a| a.to_string())
                        .unwrap_or_default()
                };
                let _ = event_tx.send(NodeEvent::PeerConnected {
                    peer_id: peer_id.to_base58(),
                    addr: addr_str,
                });
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                peer_count = peer_count.saturating_sub(1);
                log::info!("peer_node: disconnected from {peer_id} (count={peer_count})");
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                log::debug!("peer_node: listening on {address}");
            }
            _ => {}
        }

        // 连接数变化才通知（避免刷屏）
        if last_peer_count != Some(peer_count) {
            last_peer_count = Some(peer_count);
            let _ = event_tx.send(NodeEvent::PeerCountChanged(peer_count));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 多主题通道字段（契约 lan-file 6.1）：命令/事件均携带 topic，
    /// 通道结构保持「不携带业务语义」约束（仅字段级验证，swarm 行为由真机验收覆盖）。
    #[test]
    fn command_and_event_carry_topic() {
        let (tx, rx) = std::sync::mpsc::channel::<NodeCommand>();
        tx.send(NodeCommand::Publish {
            topic: "vitrytool-lan-file-announce".into(),
            data: b"{}".to_vec(),
        })
        .unwrap();
        match rx.recv() {
            Ok(NodeCommand::Publish { topic, data }) => {
                assert_eq!(topic, "vitrytool-lan-file-announce");
                assert_eq!(data, b"{}");
            }
            other => panic!("unexpected: {other:?}"),
        }

        let (tx, rx) = std::sync::mpsc::channel::<NodeEvent>();
        tx.send(NodeEvent::PubsubMessage {
            topic: "vitrytool-lan-clipboard".into(),
            source: "12D3KooTest".into(),
            data: vec![1, 2, 3],
        })
        .unwrap();
        match rx.recv() {
            Ok(NodeEvent::PubsubMessage { topic, .. }) => {
                assert_eq!(topic, "vitrytool-lan-clipboard");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
