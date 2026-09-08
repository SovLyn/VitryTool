//! lan-file 运行时共享态与任务管理（Tauri 胶水层，契约 `docs/api/lan-file.md`）。
//!
//! 结构：
//! - `LanFileShared`：设置（开关/信任表）、公告 peers、活跃交互会话槽、图片通道小队列；
//!   以 `OnceLock<Arc<Mutex<_>>>` 全局持有（与 lan-sync 同模式）。
//! - `init(app, self_peer_id, signing_key)`（setup 调用）：加载设置 → 启动 TCP 监听 →
//!   注册公告消费线程（announcements 主题）→ 注册 core::hooks 开关与图片钩子。
//! - 交互传输任务：发起方 `send_task`（校验 → dial → Offer → 等 Accept/Reject →
//!   逐文件 Chunk/End/Ack，断线指数退避重连续传）；接收方监听器 accept → 握手 →
//!   Offer → 前端确认（accept/reject 命令经 channel 送达任务）→ 落盘（.tmp→rename）。
//! - 取消墓碑 / sidecar 120s 窗口（5.5）；图片通道门控与 LRU（5.7）。
//!
//! 命令层（commands.rs）接线后多数项即被消费；`a_ip`/地址学习表见 `learn_peer_addr`。

#![allow(dead_code)]

use super::service::{
    self as svc, err, evict_expired, find_announce, retry_delay, upsert_announce, Announce,
    AnnounceEntry, FileStatus, LanFileOffer, LanFilePeer, LanFileStatus, LanFileTransfer,
    OfferFileInfo, RateEstimator, TransferError, TransferFileInfo, TransferState,
    ANNOUNCE_INTERVAL, ANNOUNCE_TOPIC, ANNOUNCE_VERSION, IMAGE_QUEUE_CAP, OFFER_TIMEOUT,
    PROGRESS_INTERVAL, RESUME_WINDOW,
};
use super::store::{
    dedup_filename, disk_space_ok, is_image_ext, load_sidecar, lru_evict_images, remove_sidecar,
    remove_tmp_file, sanitize_filename, save_sidecar, LanFileSettings, SidecarFile, SidecarMeta,
    StoreBackend, IMAGE_EXTS,
};
use super::transport::proto::{
    AckFrame, EndFrame, InitialFrame, OfferFile, ReplyFrame, CHUNK_SIZE,
};
use super::transport::session::{PeerIdentity, SecureSession, SessionError};
use crate::core::hooks;
use crate::core::notify::{notify_app, NotifyLevel};
use crate::core::state::AppState;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

/// 已点亮图片路径（lan-sync 回写用，契约 lan-file 5.8 / lan-sync 5.5）。
pub fn lit_image_path(
    app: &AppHandle,
    meta: &crate::features::lan_sync::service::ImageMeta,
) -> Option<String> {
    let hash = meta.hash.as_deref()?;
    let dir = app.path().app_data_dir().ok()?.join("lan-inbox-images");
    for ext in IMAGE_EXTS {
        let p = dir.join(format!("{hash}.{ext}"));
        if p.exists() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 事件名（契约第 2 节）
// ---------------------------------------------------------------------------

pub const PEERS_UPDATED_EVENT: &str = "lan-file://peers-updated";
pub const INCOMING_EVENT: &str = "lan-file://incoming";
pub const TRANSFER_UPDATED_EVENT: &str = "lan-file://transfer-updated";
pub const SETTINGS_UPDATED_EVENT: &str = "lan-file://settings-updated";

// ---------------------------------------------------------------------------
// 共享运行时状态
// ---------------------------------------------------------------------------

/// 图片通道队列项（契约 5.7-6：对每个在线 caps(img) 终端各入队一次）。
#[derive(Debug, Clone)]
pub struct ImageOfferJob {
    pub transfer_id: String,
    /// 图片源路径（本机已落盘的截图）。
    pub path: PathBuf,
    pub name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: u64,
    pub hash: String,
    /// 目标终端（公告快照，含 ip:port）。
    pub target: Announce,
}

/// 图片条目落盘记录（收件箱点亮查询用；v1 以目录为准，不持久化）。
pub struct LanFileShared {
    pub settings: LanFileSettings,
    /// 在线公告列表（TTL 驱逐）。
    pub announces: Vec<AnnounceEntry>,
    /// 本机 peerId。
    pub self_peer_id: String,
    /// 终端名（来自 lan-sync 设置）。
    pub terminal_name: String,
    /// 本机指纹（SHA256:base64(公钥)）。
    pub fingerprint: String,
    /// TCP 监听端口（None = 未监听）。
    pub tcp_port: Option<u16>,
    /// 监听是否健康。
    pub listening: bool,
    /// 活跃交互任务槽（同一时刻至多 1 个，契约 5.3）。
    pub active_transfer: Option<ActiveTask>,
    /// 图片通道队列（串行处理，超限丢最旧）。
    pub image_queue: VecDeque<ImageOfferJob>,
    /// 图片接收端最近拒绝的 transferId（防重复处理同一连接重试）。
    pub recent_image_offers: VecDeque<String>,
}

/// 活跃任务的命令通道（命令 → 任务 tokio 任务）。
pub enum TaskCommand {
    Accept,
    Reject,
    Cancel,
}

/// 活跃交互任务（内存态；快照经 transfer-updated 推给前端）。
pub struct ActiveTask {
    pub transfer_id: String,
    pub direction: &'static str, // "send" | "receive"
    pub peer_id: String,
    pub terminal_name: String,
    /// 任务命令入口（任务侧 Drop 时通道断开）。
    pub cmd_tx: Sender<TaskCommand>,
    /// 会话槽代数（续传顶替旧会话时递增；释放时校验，避免旧任务误清新槽）。
    pub epoch: u64,
}

/// 会话槽代数计数器。
static SESSION_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

static LAN_FILE: OnceLock<Arc<Mutex<LanFileShared>>> = OnceLock::new();
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

pub fn mark_shutting_down() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
}

/// 获取共享态（setup 后可用）。
pub fn shared() -> Option<&'static Arc<Mutex<LanFileShared>>> {
    LAN_FILE.get()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn emit_transfer(app: &AppHandle, t: &LanFileTransfer) {
    if let Err(e) = app.emit(TRANSFER_UPDATED_EVENT, t) {
        log::warn!("lan_file: emit transfer failed: {e}");
    }
}

fn idle_snapshot(direction: &str) -> LanFileTransfer {
    LanFileTransfer {
        transfer_id: None,
        direction: direction.to_string(),
        peer_id: None,
        terminal_name: None,
        state: TransferState::Done,
        files: vec![],
        total_bytes: 0,
        transferred_bytes: 0,
        bytes_per_sec: 0.0,
        saved_paths: None,
        error: None,
    }
}

// ---------------------------------------------------------------------------
// 初始化（setup）
// ---------------------------------------------------------------------------

/// 初始化 lan-file（setup 阶段调用；桌面专属——移动端仅图片接收，
/// 复用本函数但交互命令不注册，见 lib.rs）。
pub fn init(
    app: &AppHandle,
    self_peer_id: String,
    signing: ed25519_dalek::SigningKey,
) -> Result<(), String> {
    let backend = StoreBackend::new(app).map_err(|e| e.0)?;
    let settings = backend.load_settings().map_err(|e| e.0)?;

    // 启动清理（契约 5.5）：扫 `AppData/lanfile` 的 sidecar 与 `.tmp`，
    // 超过上次 mtime + 120s 的残留直接清理；窗口内的保留等发起方重连续传。
    cleanup_stale_transfer_artifacts(app);

    // 终端名与 lan-sync 同源（设置页一处改名，处处生效）
    let terminal_name = crate::features::lan_sync::state::shared()
        .map(|g| g.lock().unwrap().settings.terminal_name.clone())
        .unwrap_or_else(|| "VitryTool".into());

    let fingerprint = super::transport::crypto::fingerprint_of(&signing.verifying_key());

    // 启动 TCP 监听（0.0.0.0:0 动态端口，契约 5.2）
    // 端口经「绑定后立即回传」的 oneshot 通道获取（run_listener 的返回值只在线程
    // 退出时才有——之前误用它做初始化回传，导致 init 永远超时、listening=false、
    // 双方都不公告而互相不可见的严重 bug）。
    let (listen_tx, listen_rx) = std::sync::mpsc::channel();
    let listener_app = app.clone();
    let listen_signing = signing.clone();
    let self_pid = self_peer_id.clone();
    std::thread::Builder::new()
        .name("lan-file-listener".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("lan-file-tcp")
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("lan_file: listener runtime build failed: {e}");
                    let _ = listen_tx.send(None);
                    return;
                }
            };
            rt.block_on(run_listener(
                listener_app,
                listen_signing,
                self_pid,
                listen_tx,
            ));
        })
        .map_err(|e| format!("spawn listener thread failed: {e}"))?;
    // 等待监听端口（最多 3s）
    let tcp_port = listen_rx
        .recv_timeout(Duration::from_secs(3))
        .ok()
        .flatten();

    let state = Arc::new(Mutex::new(LanFileShared {
        settings,
        announces: Vec::new(),
        self_peer_id: self_peer_id.clone(),
        terminal_name: terminal_name.clone(),
        fingerprint: fingerprint.clone(),
        tcp_port,
        listening: tcp_port.is_some(),
        active_transfer: None,
        image_queue: VecDeque::new(),
        recent_image_offers: VecDeque::new(),
    }));
    LAN_FILE
        .set(state)
        .map_err(|_| "lan-file already initialized".to_string())?;
    let _ = SIGNING_KEY.set(signing.clone());

    log::info!("lan_file: init peer={self_peer_id} port={tcp_port:?} terminal={terminal_name}");

    // 公告消费线程：NodeEvent 已由 lan-sync 的消费者独占（单通道单消费者），
    // lan-file 另起线程做周期公告 + TTL 驱逐；公告接收经 peer_node 事件桥（见 init_announce_bridge）
    init_announce_bridge(app.clone(), signing.clone());
    start_image_worker(app.clone(), signing);

    // 托盘「文件共享」开关钩子（core::hooks，契约 quick-paste 5.5）
    hooks::register_lan_file_switches(hooks::LanFileSwitches {
        enabled: lan_file_enabled,
        set_enabled: set_enabled_from_tray,
    });

    // 启动即公告一次（其余周期由 announce bridge 承担）
    publish_announce(app);
    Ok(())
}

/// 读取总开关（未初始化返回 false）。
pub fn lan_file_enabled() -> bool {
    shared()
        .map(|g| g.lock().unwrap().settings.enabled)
        .unwrap_or(false)
}

/// 启动清理残留传输工件（契约 5.5；只删本功能命名的 sidecar / `.tmp`）。
fn cleanup_stale_transfer_artifacts(app: &AppHandle) {
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    let dir = data_dir.join("lanfile");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let artifacts: Vec<svc::TransferArtifact> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let mtime = e
                .metadata()
                .ok()?
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_millis() as u64;
            Some(svc::TransferArtifact {
                name,
                mtime_ms: mtime,
            })
        })
        .collect();
    for name in svc::stale_transfer_artifacts(&artifacts, now_ms(), RESUME_WINDOW) {
        match std::fs::remove_file(dir.join(&name)) {
            Ok(()) => log::info!("lan_file: cleaned stale artifact {name}"),
            Err(e) => log::debug!("lan_file: clean {name} failed: {e}"),
        }
    }
}

/// 托盘开关：设置并持久化，emit settings-updated（契约 5.1）。
fn set_enabled_from_tray(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    set_enabled_impl(app, enabled)?;
    Ok(enabled)
}

/// 实际开关切换（命令与托盘共用路径）：关 = 终止活跃任务 + 清空图片队列。
pub fn set_enabled_impl(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let cancel_task;
    {
        let shared = shared().ok_or_else(|| "lan-file not initialized".to_string())?;
        let mut g = shared.lock().unwrap();
        g.settings.enabled = enabled;
        cancel_task = !enabled;
        if !enabled {
            g.image_queue.clear();
        }
    }
    let backend = StoreBackend::new(app).map_err(|e| e.0)?;
    let settings = {
        let shared = shared().unwrap();
        let g = shared.lock().unwrap();
        g.settings.clone()
    };
    backend.save_settings(&settings).map_err(|e| e.0)?;
    if cancel_task {
        cancel_active(app, "disabled");
    }
    // 关闭 → 尽力发撤销公告；开启 → 立即重新公告（契约 5.1/5.2）
    if enabled {
        publish_announce(app);
    } else {
        revoke_announce(app);
    }
    let _ = app.emit(
        SETTINGS_UPDATED_EVENT,
        serde_json::json!({ "fileShare": enabled }),
    );
    log::info!("lan_file: enabled={enabled}");
    Ok(())
}

/// 发布本机公告（启动 / 周期 / 发现新对端后补发）。总开关关闭时不公告（契约 5.1）。
pub fn publish_announce(app: &AppHandle) {
    let (enabled, port, self_pid, terminal, fingerprint) = {
        let Some(shared) = shared() else { return };
        let g = shared.lock().unwrap();
        if !g.listening {
            return;
        }
        (
            g.settings.enabled,
            g.tcp_port,
            g.self_peer_id.clone(),
            g.terminal_name.clone(),
            g.fingerprint.clone(),
        )
    };
    if !enabled {
        return;
    }
    let Some(port) = port else { return };
    // caps：桌面 = file + img（移动端 init 传入仅 img，见 lib.rs；桌面恒二者）
    let caps = if cfg!(desktop) {
        vec!["file".to_string(), "img".to_string()]
    } else {
        vec!["img".to_string()]
    };
    let announce = Announce {
        v: ANNOUNCE_VERSION.to_string(),
        peer_id: self_pid,
        terminal,
        tcp_port: port,
        fingerprint,
        caps,
        ts: now_ms(),
    };
    let Ok(bytes) = serde_json::to_vec(&announce) else {
        return;
    };
    publish_announce_bytes(app, &bytes, port);
}

/// 尽力发撤销公告（关闭开关 / 退出时；`caps` 为空 = 撤销，契约 5.2）。
///
/// 发不出去也无妨：接收侧 12 分钟 TTL 自然驱逐。
pub fn revoke_announce(app: &AppHandle) {
    let Some(shared) = shared() else { return };
    let (self_pid, terminal, fingerprint, port) = {
        let g = shared.lock().unwrap();
        (
            g.self_peer_id.clone(),
            g.terminal_name.clone(),
            g.fingerprint.clone(),
            g.tcp_port.unwrap_or(0),
        )
    };
    let announce = Announce {
        v: ANNOUNCE_VERSION.to_string(),
        peer_id: self_pid,
        terminal,
        tcp_port: port,
        fingerprint,
        caps: Vec::new(), // 撤销标记
        ts: now_ms(),
    };
    let Ok(bytes) = serde_json::to_vec(&announce) else {
        return;
    };
    publish_announce_bytes(app, &bytes, port);
    log::info!("lan_file: revoke announce published (caps empty)");
}

fn publish_announce_bytes(app: &AppHandle, bytes: &[u8], port: u16) {
    let state = app.state::<AppState>();
    let node = state.peer_node.lock().unwrap();
    if let Some(node) = node.as_ref() {
        node.publish(ANNOUNCE_TOPIC, bytes.to_vec());
        log::debug!("lan_file: announce published (port={port})");
    } else {
        log::warn!("lan_file: peer_node not running, announce skipped");
    }
}

/// 公告最小间隔（**仅用于「收到对端公告后回发」这一条路径**的回声兜底）。
const ANNOUNCE_MIN_INTERVAL_MS: u64 = 3_000;
/// 上次公告时刻（unix 毫秒；0 = 从未）。
static LAST_ANNOUNCE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 公告节流：**只**对「收到对端公告后回发自身公告」生效。
///
/// 注意不能给所有公告加节流——启动公告与「连接建立后立即补发」是发现的关键路径，
/// 被节流吞掉会导致双方互不可见直到 5 分钟周期公告（真机实测）。
fn announce_rate_ok() -> bool {
    let now = now_ms();
    let last = LAST_ANNOUNCE_MS.load(Ordering::SeqCst);
    if now.saturating_sub(last) < ANNOUNCE_MIN_INTERVAL_MS {
        return false;
    }
    LAST_ANNOUNCE_MS.store(now, Ordering::SeqCst);
    true
}

/// 收到对端公告后回发自身公告（带回声节流；首次发现新对端时调用）。
fn publish_announce_reply(app: &AppHandle) {
    if !announce_rate_ok() {
        log::debug!("lan_file: announce reply throttled");
        return;
    }
    publish_announce(app);
}

/// 处理一条收到的公告（announcements 主题）。
pub fn handle_announce(app: &AppHandle, source: &str, data: &[u8]) {
    let Ok(announce) = serde_json::from_slice::<Announce>(data) else {
        log::debug!("lan_file: unparsable announce from {source}");
        return;
    };
    // 公告自校验（契约 5.2：multihash(公钥)==peerId 不一致即丢弃）
    if !svc::announce_consistent(&announce) {
        log::warn!(
            "lan_file: announce binding mismatch from {}",
            announce.peer_id
        );
        return;
    }
    if announce.peer_id == source {
        // source 即 gossipsub propagation_source（直连对端）——一致
    }
    let changed;
    let newly_seen;
    {
        let Some(shared) = shared() else { return };
        let mut g = shared.lock().unwrap();
        if announce.peer_id == g.self_peer_id {
            return; // 自己的公告
        }
        if svc::announce_is_stale(&g.announces, &announce) {
            // 乱序旧公告（含旧实例的撤销公告）：忽略，避免误删已上线对端
            log::debug!(
                "lan_file: stale announce ignored (peer={} ts={})",
                announce.peer_id,
                announce.ts
            );
            return;
        }
        if svc::is_revoke_announce(&announce) {
            // 撤销公告（对端关闭开关 / 退出）：从列表移除（契约 5.2）
            changed = svc::remove_announce(&mut g.announces, &announce.peer_id);
            newly_seen = false;
            if changed {
                log::info!("lan_file: peer {} revoked announce", announce.peer_id);
            }
        } else {
            // 是否首次见到该对端（决定是否补发自身公告）
            newly_seen = svc::announce_back_needed(&g.announces, &announce.peer_id);
            upsert_announce(&mut g.announces, announce, now_ms());
            changed = true;
        }
    }
    if changed {
        let _ = app.emit(PEERS_UPDATED_EVENT, ());
    }
    // 补发自身公告**仅限首次发现该对端**（契约 5.2「mDNS 发现新对端后补公告」）。
    // 反例（真机实测到的严重 bug）：对每条收到的公告都补发 → 两端互相触发，
    // gossipsub 每条消息的 seqno 不同、去重失效 → 公告回声风暴（每端数十条/秒）。
    if newly_seen {
        publish_announce_reply(app);
    }
}

/// 公告桥：消费 peer_node 事件中的公告主题消息 + 周期重公告 + TTL 驱逐。
///
/// peer_node 事件通道由 lan-sync 消费者独占；lan-sync 消费者把非剪贴板主题
/// 的事件转投本桥通道（`forward_node_event`）。
static ANNOUNCE_TX: OnceLock<Sender<(String, Vec<u8>)>> = OnceLock::new();

/// lan-sync 消费者转发非剪贴板主题消息（fire-and-forget）。
pub fn forward_announce(source: &str, data: &[u8]) {
    if let Some(tx) = ANNOUNCE_TX.get() {
        let _ = tx.send((source.to_string(), data.to_vec()));
    }
}

/// 从 multiaddr（如 `/ip4/192.168.31.203/tcp/12345` 或
/// `/ip4/192.168.31.203/udp/52981/quic-v1/...`）提取 IPv4/IPv6 地址；
/// 提取失败返回空串（VLF 数据面 dial 将走 `ip:port`）。
pub fn ip_from_multiaddr(addr: &str) -> String {
    let segs: Vec<&str> = addr.split('/').collect();
    for (i, seg) in segs.iter().enumerate() {
        if (*seg == "ip4" || *seg == "ip6") && i + 1 < segs.len() {
            return segs[i + 1].to_string();
        }
    }
    String::new()
}

/// 与新 peer 建立连接（PeerConnected 事件）：学习对端 IP（VLF 数据面 dial 用）+
/// 立即补发公告 + 2s 延迟再补一次（订阅握手缓冲，契约 5.2「发现新对端后补公告」）。
pub fn on_peer_connected(app: &AppHandle, peer_id: &str, addr: &str) {
    let ip = ip_from_multiaddr(addr);
    if !ip.is_empty() {
        learn_peer_addr(peer_id, &ip);
        log::info!("lan_file: learned peer {peer_id} ip={ip}");
    }
    publish_announce(app);
    let app2 = app.clone();
    std::thread::Builder::new()
        .name("lan-file-reannounce".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            publish_announce(&app2);
        })
        .ok();
}

fn init_announce_bridge(app: AppHandle, signing: ed25519_dalek::SigningKey) {
    let (tx, rx) = std::sync::mpsc::channel::<(String, Vec<u8>)>();
    let _ = ANNOUNCE_TX.set(tx);
    let _ = signing; // 预留：公告消费侧无需签名
    std::thread::Builder::new()
        .name("lan-file-announce".into())
        .spawn(move || {
            let mut last_announce = SystemTime::now();
            loop {
                if SHUTTING_DOWN.load(Ordering::SeqCst) {
                    break;
                }
                // 收公告（50ms 轮询兼顾周期任务）
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok((source, data)) => handle_announce(&app, &source, &data),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
                // 周期公告（5min）
                if last_announce.elapsed().unwrap_or_default() >= ANNOUNCE_INTERVAL {
                    last_announce = SystemTime::now();
                    publish_announce(&app);
                }
                // TTL 驱逐（12min）
                {
                    let Some(shared) = shared() else { continue };
                    let mut g = shared.lock().unwrap();
                    if evict_expired(&mut g.announces, now_ms()) {
                        drop(g);
                        let _ = app.emit(PEERS_UPDATED_EVENT, ());
                    }
                }
            }
            log::debug!("lan_file: announce bridge stopped");
        })
        .ok();
}

// ---------------------------------------------------------------------------
// TCP 监听器（接收方入口）
// ---------------------------------------------------------------------------

/// 监听 0.0.0.0:0，循环 accept → 握手 → 分发（交互 Offer / ImageOffer / ResumeOffer）。
/// 返回监听端口（线程退出时 None）。
async fn run_listener(
    app: AppHandle,
    signing: ed25519_dalek::SigningKey,
    self_peer_id: String,
    port_tx: std::sync::mpsc::Sender<Option<u16>>,
) {
    let listener = match tokio::net::TcpListener::bind("0.0.0.0:0").await {
        Ok(l) => l,
        Err(e) => {
            log::error!("lan_file: tcp listen failed: {e}");
            let _ = port_tx.send(None);
            return;
        }
    };
    let port = listener.local_addr().ok().map(|a| a.port());
    // 绑定成功立即回传端口（init 据此置 listening=true 并公告）
    let _ = port_tx.send(port);
    log::info!("lan_file: listening on 0.0.0.0:{port:?}");
    loop {
        if SHUTTING_DOWN.load(Ordering::SeqCst) {
            break;
        }
        let (stream, addr) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                log::debug!("lan_file: accept failed: {e}");
                continue;
            }
        };
        log::debug!("lan_file: inbound connection from {addr}");
        let app = app.clone();
        let signing = signing.clone();
        let self_peer_id = self_peer_id.clone();
        tokio::spawn(async move {
            let peer_enabled = shared()
                .map(|g| g.lock().unwrap().settings.enabled)
                .unwrap_or(false);
            match SecureSession::accept(stream, &signing, &self_peer_id).await {
                Ok((mut session, peer)) => {
                    if !peer_enabled {
                        log::info!(
                            "lan_file: inbound from {} rejected (disabled)",
                            peer.peer_id
                        );
                        let _ = session
                            .send_json(&ReplyFrame::Reject {
                                code: err::NOT_ENABLED.into(),
                            })
                            .await;
                        session.shutdown().await;
                        return;
                    }
                    if let Err(e) = handle_inbound(app, session, peer).await {
                        log::info!("lan_file: inbound session ended: {e}");
                    }
                }
                Err(e) => log::debug!("lan_file: inbound handshake failed from {addr}: {e}"),
            }
        });
    }
}

/// 分发入站会话：Initial 帧决定路径（交互 / 图片 / 续传重连）。
async fn handle_inbound(
    app: AppHandle,
    mut session: SecureSession,
    peer: PeerIdentity,
) -> Result<(), SessionError> {
    let initial: InitialFrame = session.recv_json().await?;
    match initial {
        InitialFrame::ImageOffer {
            transfer_id,
            name,
            width,
            height,
            size,
            hash,
        } => {
            receive_image(
                app,
                session,
                peer,
                transfer_id,
                name,
                width,
                height,
                size,
                hash,
            )
            .await
        }
        InitialFrame::Offer {
            transfer_id,
            files,
            total_bytes,
        } => receive_interactive(app, session, peer, transfer_id, files, total_bytes, false).await,
        InitialFrame::ResumeOffer {
            transfer_id,
            files,
            total_bytes,
        } => receive_interactive(app, session, peer, transfer_id, files, total_bytes, true).await,
    }
}

/// 取活跃任务槽（互斥占用，契约 5.3 单会话）；返回本会话的 epoch（释放时校验用）。
///
/// `preempt_same_id`：**续传重连**时允许顶掉同一 transferId 的旧会话——对端链路
/// 停滞重连时，本端旧会话任务可能还阻塞在读里（数据面无帧级硬超时），
/// 不顶掉就会把合法的续传请求拒成 `busy`（真机实测：卡顿恢复后传输直接失败）。
fn try_occupy_session(
    transfer_id: &str,
    direction: &'static str,
    peer_id: &str,
    terminal_name: &str,
    cmd_tx: Sender<TaskCommand>,
    preempt_same_id: bool,
) -> Option<u64> {
    let shared = shared()?;
    let mut g = shared.lock().unwrap();
    if let Some(existing) = g.active_transfer.as_ref() {
        if !(preempt_same_id && existing.transfer_id == transfer_id) {
            return None;
        }
        log::info!("lan_file: preempting stale session for {transfer_id} (resume reconnect)");
    }
    let epoch = SESSION_EPOCH.fetch_add(1, Ordering::SeqCst);
    g.active_transfer = Some(ActiveTask {
        transfer_id: transfer_id.to_string(),
        direction,
        peer_id: peer_id.to_string(),
        terminal_name: terminal_name.to_string(),
        cmd_tx,
        epoch,
    });
    Some(epoch)
}

/// 释放会话槽（**仅当 epoch 匹配**：避免被顶掉的旧任务回来误清新会话的槽）。
fn release_session(transfer_id: &str, epoch: u64) {
    if let Some(shared) = shared() {
        let mut g = shared.lock().unwrap();
        if g.active_transfer
            .as_ref()
            .map(|t| t.transfer_id == transfer_id && t.epoch == epoch)
            .unwrap_or(false)
        {
            g.active_transfer = None;
        }
    }
}

/// 通知前端回到空闲（`transferId` 省略）。任务进入终态时**不发**（终态快照停留展示），
/// 仅用于任务未能启动（运行时/线程失败）等无终态路径，避免前端卡住一张假卡片。
pub fn emit_idle(app: &AppHandle, direction: &str) {
    emit_transfer(app, &idle_snapshot(direction));
}

/// 终态快照参数（done / failed / cancelled / rejected）。
///
/// 契约 5.3：任务进入终态后**停留展示**（失败可重试、接收完成可打开所在位置），
/// 不再紧跟 idle 快照（idle 会立刻抹掉卡片，前端只见「点一下什么都没了」）。
struct TerminalSnapshot<'a> {
    transfer_id: &'a str,
    direction: &'a str,
    peer_id: &'a str,
    terminal_name: &'a str,
    state: TransferState,
    files: Vec<TransferFileInfo>,
    total_bytes: u64,
    transferred_bytes: u64,
    saved_paths: Option<Vec<String>>,
    error: Option<String>,
}

fn emit_terminal(app: &AppHandle, s: TerminalSnapshot<'_>) {
    emit_transfer(
        app,
        &LanFileTransfer {
            transfer_id: Some(s.transfer_id.to_string()),
            direction: s.direction.to_string(),
            peer_id: Some(s.peer_id.to_string()),
            terminal_name: Some(s.terminal_name.to_string()),
            state: s.state,
            files: s.files,
            total_bytes: s.total_bytes,
            transferred_bytes: s.transferred_bytes,
            bytes_per_sec: 0.0,
            saved_paths: s.saved_paths,
            error: s.error.map(|code| TransferError { code, params: None }),
        },
    );
}

/// 用户显式取消（任一侧）：任务通道 Cancel + 墓碑 + 清理（契约 5.5）。
pub fn cancel_active(_app: &AppHandle, reason: &str) {
    let (cmd_tx, transfer_id) = {
        let Some(shared) = shared() else { return };
        let g = shared.lock().unwrap();
        match g.active_transfer.as_ref() {
            Some(t) => (Some(t.cmd_tx.clone()), t.transfer_id.clone()),
            None => (None, String::new()),
        }
    };
    if let Some(tx) = cmd_tx {
        let _ = tx.send(TaskCommand::Cancel);
        log::info!("lan_file: user cancel requested ({reason}) id={transfer_id}");
    }
}

/// 查询活跃任务 transferId（前端取消按钮定位用）。
pub fn active_transfer_id() -> Option<(String, &'static str)> {
    shared().and_then(|g| {
        let g = g.lock().unwrap();
        g.active_transfer
            .as_ref()
            .map(|t| (t.transfer_id.clone(), t.direction))
    })
}

/// TOFU 确认（契约 5.4）：写入信任表 + 持久化。
pub fn trust_peer(app: &AppHandle, peer_id: &str, terminal_name: &str) -> Result<(), String> {
    let shared = shared().ok_or("lan-file not initialized")?;
    {
        let mut g = shared.lock().unwrap();
        g.settings.trust(peer_id, terminal_name, now_iso());
    }
    let settings = {
        let g = shared.lock().unwrap();
        g.settings.clone()
    };
    StoreBackend::new(app)
        .and_then(|b| b.save_settings(&settings))
        .map_err(|e| e.0)?;
    Ok(())
}

/// 移除信任（设置页「已信任终端」；不影响进行中会话，契约 5.4）。
pub fn untrust_peer(app: &AppHandle, peer_id: &str) -> Result<bool, String> {
    let shared = shared().ok_or("lan-file not initialized")?;
    let removed = {
        let mut g = shared.lock().unwrap();
        g.settings.untrust(peer_id)
    };
    if removed {
        let settings = {
            let g = shared.lock().unwrap();
            g.settings.clone()
        };
        StoreBackend::new(app)
            .and_then(|b| b.save_settings(&settings))
            .map_err(|e| e.0)?;
    }
    Ok(removed)
}

/// peers 快照（getLanFilePeers 响应，契约 3）。
pub fn peers_snapshot(app: &AppHandle) -> Vec<LanFilePeer> {
    let Some(shared) = shared() else {
        return vec![];
    };
    let g = shared.lock().unwrap();
    let trusted: Vec<bool> = g
        .announces
        .iter()
        .map(|e| g.settings.is_trusted(&e.announce.peer_id))
        .collect();
    let _ = app;
    g.announces
        .iter()
        .enumerate()
        .map(|(i, e)| LanFilePeer {
            peer_id: e.announce.peer_id.clone(),
            terminal_name: e.announce.terminal.clone(),
            fingerprint: e.announce.fingerprint.clone(),
            trusted: trusted[i],
            supports_interactive: e.announce.supports_interactive(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 任务上下文与提议决定（commands 层入口）
// ---------------------------------------------------------------------------

/// 签名密钥（init 存入；VLF 握手复用身份密钥）。
static SIGNING_KEY: OnceLock<ed25519_dalek::SigningKey> = OnceLock::new();

/// 供命令层获取任务上下文（签名密钥 + 本机 peerId）。
pub fn task_context(app: &AppHandle) -> Option<(ed25519_dalek::SigningKey, String)> {
    let _ = app;
    let signing = SIGNING_KEY.get()?.clone();
    let self_peer_id = shared()?.lock().unwrap().self_peer_id.clone();
    Some((signing, self_peer_id))
}

/// 待决提议（transferId → 来源信息 + 决定通道）。
struct PendingOffer {
    peer_id: String,
    terminal_name: String,
    cmd_tx: Sender<TaskCommand>,
}

static PENDING_OFFERS: OnceLock<Mutex<std::collections::HashMap<String, PendingOffer>>> =
    OnceLock::new();

fn pending_offers() -> &'static Mutex<std::collections::HashMap<String, PendingOffer>> {
    PENDING_OFFERS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 注册待决提议（接收任务发出 lan-file://incoming 后调用）。
pub fn register_pending_offer(
    transfer_id: &str,
    peer_id: &str,
    terminal_name: &str,
    cmd_tx: Sender<TaskCommand>,
) {
    pending_offers().lock().unwrap().insert(
        transfer_id.to_string(),
        PendingOffer {
            peer_id: peer_id.to_string(),
            terminal_name: terminal_name.to_string(),
            cmd_tx,
        },
    );
}

/// 查询待决提议来源（accept 命令的 TOFU 写表用）。
pub fn pending_offer_peer(transfer_id: &str) -> Option<(String, String)> {
    let map = pending_offers().lock().unwrap();
    map.get(transfer_id)
        .map(|p| (p.peer_id.clone(), p.terminal_name.clone()))
}

/// 移除待决表项（超时 / 任务结束的兜底清理；决定已送达时表项已被 `resolve_offer` 移除）。
pub fn unregister_pending_offer(transfer_id: &str) {
    pending_offers().lock().unwrap().remove(transfer_id);
}

/// 送达决定（accept / reject）；移除待决表项。
pub fn resolve_offer(transfer_id: &str, cmd: TaskCommand) {
    let tx = pending_offers()
        .lock()
        .unwrap()
        .remove(transfer_id)
        .map(|p| p.cmd_tx);
    if let Some(tx) = tx {
        let _ = tx.send(cmd);
    }
}

/// 用户取消（契约 5.5：显式取消 = 永久终止）。
///
/// 三条路径，按「当前任务形态」处理——**任何一条都必须让前端卡片收束**，
/// 否则用户点「取消传输」会看到「命令成功但界面毫无反应」（真机实测）：
/// 1. 待决提议（接收方在等用户决定）→ 命令通道投递 Cancel；
/// 2. 活跃任务（发送/接收中）→ 命令通道投递 Cancel（任务内发 Cancel 帧 / 写墓碑）；
/// 3. 任务已随连接结束退出但 **sidecar 还在续传窗口内**（卡片显示 resuming）→
///    本函数直接写墓碑、删 `.tmp`、删 sidecar，并推终态快照；
/// 4. 全都没有（事件丢失导致的残留卡片）→ 推一个 cancelled 快照收束 UI。
pub fn cancel_transfer(app: &AppHandle, transfer_id: &str) {
    if transfer_id.is_empty() {
        return;
    }
    // 1) 待决提议
    resolve_offer(transfer_id, TaskCommand::Cancel);
    // 2) 活跃任务
    let active = {
        let Some(shared) = shared() else { return };
        let g = shared.lock().unwrap();
        g.active_transfer
            .as_ref()
            .filter(|t| t.transfer_id == transfer_id)
            .map(|t| t.cmd_tx.clone())
    };
    if let Some(tx) = active {
        let _ = tx.send(TaskCommand::Cancel);
        log::info!("lan_file: cancel delivered to active task {transfer_id}");
        return;
    }
    // 3) 续传窗口内的接收侧残留（任务已退出，只有 sidecar/.tmp）
    let data_dir = app.path().app_data_dir().unwrap_or_default();
    let inbox_dir = data_dir.join("lanfile");
    if let Some(meta) = load_sidecar(&inbox_dir, transfer_id).unwrap_or(None) {
        for f in &meta.files {
            remove_tmp_file(&inbox_dir, &f.tmp_name);
        }
        let _ = remove_sidecar(&inbox_dir, transfer_id);
        let files: Vec<TransferFileInfo> = meta
            .files
            .iter()
            .map(|f| TransferFileInfo {
                name: sanitize_filename(&f.name),
                size: 0,
                transferred_bytes: f.received_bytes,
                status: FileStatus::Transferring,
            })
            .collect();
        let transferred: u64 = meta.files.iter().map(|f| f.received_bytes).sum();
        emit_terminal(
            app,
            TerminalSnapshot {
                transfer_id,
                direction: "receive",
                peer_id: &meta.peer_id,
                terminal_name: &peer_terminal(&meta.peer_id),
                state: TransferState::Cancelled,
                files,
                total_bytes: transferred,
                transferred_bytes: transferred,
                saved_paths: None,
                error: None,
            },
        );
        notify_app(app, NotifyLevel::Info, err::CANCELLED);
        log::info!("lan_file: cancelled resumable transfer {transfer_id} (sidecar cleaned)");
        return;
    }
    // 4) 无任务无 sidecar：卡片可能是漏事件残留 → 推空闲收束
    log::info!("lan_file: cancel for inactive transfer {transfer_id} (card resolved)");
    emit_transfer(app, &idle_snapshot("send"));
}

/// 状态快照（getLanFileStatus 响应）。
pub fn status_snapshot() -> LanFileStatus {
    let Some(shared) = shared() else {
        return LanFileStatus {
            enabled: false,
            listening: false,
            tcp_port: None,
            peer_count: 0,
            trusted_count: 0,
        };
    };
    let g = shared.lock().unwrap();
    LanFileStatus {
        enabled: g.settings.enabled,
        listening: g.listening,
        tcp_port: g.tcp_port,
        peer_count: g.announces.len(),
        trusted_count: g.settings.trusted_peers.len(),
    }
}

// ---------------------------------------------------------------------------
// 发送方任务（sendLanFile 命令入口；契约 5.5 数据面 + 5.6 发送侧校验）
// ---------------------------------------------------------------------------

/// 发起传输任务（在 tokio 运行时内执行；命令层经 spawn_blocking 调用）。
///
/// 校验（5.6）→ 占会话槽 → dial → Offer → 等 Accept/Reject（60s）→
/// 逐文件 Chunk/End/Ack（多文件串行）→ done。
/// 断线（本端未取消）→ sidecar 窗口内指数退避重连（resumeHint，同 transferId）。
pub async fn send_task(
    app: AppHandle,
    signing: ed25519_dalek::SigningKey,
    self_peer_id: String,
    peer_id: String,
    paths: Vec<String>,
    transfer_id: String,
) -> Result<Vec<String>, String> {
    // 发送侧校验（5.6，全部满足才建任务）
    let total = svc::validate_send_paths(&paths, &svc::probe_regular_file)?;
    let mut metas: Vec<OfferFileInfo> = Vec::with_capacity(paths.len());
    for p in &paths {
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        metas.push(OfferFileInfo {
            name: sanitize_filename(
                &Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default(),
            ),
            size,
        });
    }

    // 占会话槽
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<TaskCommand>();
    let (terminal_name, peer_trusted) = {
        let Some(shared) = shared() else {
            return Err("lan-file not initialized".into());
        };
        let g = shared.lock().unwrap();
        let Some(entry) = g.announces.iter().find(|e| e.announce.peer_id == peer_id) else {
            return Err(err::PEER_NOT_FOUND.into());
        };
        if !entry.announce.supports_interactive() {
            return Err(err::PEER_UNSUPPORTED.into());
        }
        let trusted = g.settings.is_trusted(&peer_id);
        (entry.announce.terminal.clone(), trusted)
    };
    // dial 地址：IP 来自 PeerConnected 学习表（mdns multiaddr），端口来自公告。
    // 注意必须在占会话槽**之前**解析——失败即返回，否则槽位泄漏（永久 busy）。
    let addr = {
        let Some(shared) = shared() else {
            return Err("lan-file not initialized".into());
        };
        let g = shared.lock().unwrap();
        let ip = peer_addr(&peer_id).ok_or_else(|| err::PEER_NOT_FOUND.to_string())?;
        let port = find_announce(&g.announces, &peer_id)
            .map(|a| a.tcp_port)
            .ok_or_else(|| err::PEER_NOT_FOUND.to_string())?;
        format!("{ip}:{port}")
    };

    let Some(epoch) = try_occupy_session(
        &transfer_id,
        "send",
        &peer_id,
        &terminal_name,
        cmd_tx.clone(),
        false,
    ) else {
        return Err(err::BUSY.into());
    };
    // 立即推 offering 快照：dial 可能失败并退避重试（最多 63s），
    // 期间前端必须有卡片可看（否则「点了发送什么都没发生」）。
    emit_transfer(
        &app,
        &progress_snapshot(
            &transfer_id,
            "send",
            &peer_id,
            &terminal_name,
            &metas,
            &vec![0; metas.len()],
            total,
            0.0,
            TransferState::Offering,
            None,
        ),
    );

    let (result, transferred) = send_task_inner(
        app.clone(),
        signing,
        self_peer_id.clone(),
        peer_id.clone(),
        terminal_name.clone(),
        addr,
        paths,
        metas.clone(),
        total,
        transfer_id.clone(),
        cmd_rx,
    )
    .await;

    release_session(&transfer_id, epoch);
    // 终态快照（契约 5.3：done 发送侧摘要 / failed·cancelled·rejected 停留展示可重试）
    let (state, error_code) = match &result {
        Ok(()) => (TransferState::Done, None),
        Err(e) => {
            let peer_cancel = e.starts_with(svc::PEER_CANCEL_MARK);
            let code = svc::stable_error_code(e);
            let err_field = if code == err::CANCELLED && !peer_cancel {
                // 本地取消：状态已表达，不显示「对方取消了传输」
                None
            } else {
                Some(code.clone())
            };
            (svc::send_terminal_state(&code), err_field)
        }
    };
    emit_terminal(
        &app,
        TerminalSnapshot {
            transfer_id: &transfer_id,
            direction: "send",
            peer_id: &peer_id,
            terminal_name: &terminal_name,
            state,
            files: send_files_snapshot(&metas, &transferred, result.is_ok()),
            total_bytes: total,
            transferred_bytes: if result.is_ok() {
                total
            } else {
                total_progress(&metas, &transferred)
            },
            saved_paths: None,
            error: error_code.clone(),
        },
    );
    match &result {
        Ok(()) => {
            notify_app(&app, NotifyLevel::Success, "lan_file.done");
        }
        Err(e) if e.contains(err::CANCELLED) => {
            notify_app(&app, NotifyLevel::Info, err::CANCELLED);
        }
        Err(e) => {
            log::warn!("lan_file: send failed: {e}");
            notify_app(
                &app,
                NotifyLevel::Warning,
                &error_code.unwrap_or_else(|| err::TRANSFER_FAILED.to_string()),
            );
        }
    }
    let _ = peer_trusted;
    result.map(|_| vec![])
}

/// 发送侧文件进度快照（终态卡展示用）。
fn send_files_snapshot(
    metas: &[OfferFileInfo],
    transferred: &[u64],
    all_done: bool,
) -> Vec<TransferFileInfo> {
    metas
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let done = if all_done {
                m.size
            } else {
                transferred.get(i).copied().unwrap_or(0).min(m.size)
            };
            TransferFileInfo {
                name: m.name.clone(),
                size: m.size,
                transferred_bytes: done,
                status: if done >= m.size {
                    FileStatus::Done
                } else if done > 0 {
                    FileStatus::Transferring
                } else {
                    FileStatus::Pending
                },
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn send_task_inner(
    app: AppHandle,
    signing: ed25519_dalek::SigningKey,
    self_peer_id: String,
    peer_id: String,
    terminal_name: String,
    addr: String,
    paths: Vec<String>,
    metas: Vec<OfferFileInfo>,
    total: u64,
    transfer_id: String,
    cmd_rx: Receiver<TaskCommand>,
) -> (Result<(), String>, Vec<u64>) {
    let mut transferred: Vec<u64> = vec![0; metas.len()];
    let result = send_loop(
        app,
        signing,
        self_peer_id,
        peer_id,
        terminal_name,
        addr,
        paths,
        metas,
        total,
        transfer_id,
        cmd_rx,
        &mut transferred,
    )
    .await;
    (result, transferred)
}

#[allow(clippy::too_many_arguments)]
async fn send_loop(
    app: AppHandle,
    signing: ed25519_dalek::SigningKey,
    self_peer_id: String,
    peer_id: String,
    terminal_name: String,
    addr: String,
    paths: Vec<String>,
    metas: Vec<OfferFileInfo>,
    total: u64,
    transfer_id: String,
    cmd_rx: Receiver<TaskCommand>,
    transferred: &mut Vec<u64>,
) -> Result<(), String> {
    let mut attempt: u32 = 0u32;

    // 任务快照推送（节流 ≤4/s）
    let mut rate = RateEstimator::new();
    let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);

    loop {
        // 每次（重）连都重新解析对端地址：接收方重启会换动态端口，而 peerId 才是身份
        // （契约 5.2「重启换端口可接受」）。用首次解析的地址重连会永久 dial 失败。
        let target = resolve_peer_addr(&peer_id).unwrap_or_else(|| addr.clone());
        let connect = SecureSession::connect(&target, &signing, &self_peer_id).await;
        let (mut session, _peer) = match connect {
            Ok(v) => v,
            Err(SessionError::Io(_e)) => {
                // dial 失败：窗口内退避重试
                if attempt >= svc::RETRY_BACKOFF_SECS.len() as u32
                    || total_elapsed_exceeded(attempt)
                {
                    return Err(err::TRANSFER_FAILED.into());
                }
                // 退避期间给前端 resuming 反馈（契约 F4 琥珀横幅「网络中断，等待恢复…」）
                emit_transfer(
                    &app,
                    &progress_snapshot(
                        &transfer_id,
                        "send",
                        &peer_id,
                        &terminal_name,
                        &metas,
                        transferred,
                        total,
                        0.0,
                        TransferState::Resuming,
                        None,
                    ),
                );
                // 退避期间**轮询取消**（分片睡眠）：否则用户点「取消传输」要等到
                // 退避结束（最长 32s）才有反应，观感就是「点了没反应」（真机实测）。
                if wait_with_cancel(&cmd_rx, retry_delay(attempt)).await {
                    return Err(err::CANCELLED.into());
                }
                attempt += 1;
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };

        // Initial 帧：首连 Offer / 续传 ResumeOffer
        let is_resume = attempt > 0;
        let initial = if is_resume {
            InitialFrame::ResumeOffer {
                transfer_id: transfer_id.clone(),
                files: metas
                    .clone()
                    .into_iter()
                    .map(|f| OfferFile {
                        name: f.name,
                        size: f.size,
                    })
                    .collect(),
                total_bytes: total,
            }
        } else {
            InitialFrame::Offer {
                transfer_id: transfer_id.clone(),
                files: metas
                    .clone()
                    .into_iter()
                    .map(|f| OfferFile {
                        name: f.name,
                        size: f.size,
                    })
                    .collect(),
                total_bytes: total,
            }
        };
        if let Err(e) = session.send_json(&initial).await {
            return Err(format!("{}: {e}", err::TRANSFER_FAILED));
        }
        // 提议已发出：offering（首连）/ resuming（续传）快照（契约 5.3 状态机）
        emit_transfer(
            &app,
            &progress_snapshot(
                &transfer_id,
                "send",
                &peer_id,
                &terminal_name,
                &metas,
                transferred,
                total,
                0.0,
                if is_resume {
                    TransferState::Resuming
                } else {
                    TransferState::Offering
                },
                None,
            ),
        );

        // 命令轮询（cancel）+ 应答等待（60s 提议窗口）
        let reply = wait_reply(&mut session, &cmd_rx, OFFER_TIMEOUT).await;
        let reply = match reply {
            Ok(r) => r,
            Err(WaitError::Cancelled) => {
                // 用户取消：发 Cancel 帧 + 本地无墓碑（发送侧无落盘）
                session.send_cancel("user").await;
                return Err(err::CANCELLED.into());
            }
            Err(WaitError::PeerCancelled) => {
                // 对端显式取消（等待应答期间）：不再续传，直接终态
                return Err(svc::PEER_CANCEL_MARK.into());
            }
            Err(WaitError::Timeout) => {
                session.send_cancel("offer_timeout").await;
                return Err(err::OFFER_TIMEOUT.into());
            }
            Err(WaitError::Session(e)) => return Err(format!("{}: {e}", err::TRANSFER_FAILED)),
        };
        match reply {
            ReplyFrame::Reject { code } => {
                // 续传时被拒 busy：对端旧会话可能还占着槽（它的无数据看门狗最多 60s 才放行）
                // → 当作可重试的链路问题，退避后再来，而不是直接判失败。
                if is_resume && code == err::BUSY {
                    log::warn!(
                        "lan_file: resume rejected busy (peer session not freed yet), retry attempt={attempt}"
                    );
                    attempt += 1;
                    if total_elapsed_exceeded(attempt) {
                        return Err(err::BUSY.into());
                    }
                    if wait_with_cancel(&cmd_rx, retry_delay(attempt - 1)).await {
                        return Err(err::CANCELLED.into());
                    }
                    continue;
                }
                // 对端拒绝：磁盘满 / 忙 / 已取消墓碑等（契约 5.5）
                return Err(code);
            }
            ReplyFrame::Accept {
                fresh,
                per_file_received_bytes,
            } => {
                if !fresh {
                    if let Some(offsets) = per_file_received_bytes {
                        *transferred = offsets;
                    }
                }
            }
        }
        // 对端已接受：立即推一次 transferring（小文件可能不足一个进度节流周期）
        emit_transfer(
            &app,
            &progress_snapshot(
                &transfer_id,
                "send",
                &peer_id,
                &terminal_name,
                &metas,
                transferred,
                total,
                0.0,
                TransferState::Transferring,
                None,
            ),
        );

        // 逐文件传输（串行）
        let mut file_result: Result<(), String> = Ok(());
        for (idx, path) in paths.iter().enumerate() {
            let skip = transferred.get(idx).copied().unwrap_or(0);
            file_result = send_file_stream(
                &app,
                &mut session,
                &cmd_rx,
                idx as u32,
                path,
                &metas[idx],
                skip,
                &mut rate,
                &mut last_emit,
                &transfer_id,
                &peer_id,
                &terminal_name,
                &metas,
                transferred,
                total,
            )
            .await;
            if let Err(_e) = &file_result {
                break;
            }
        }

        match file_result {
            Ok(()) => {
                // 全部完成
                return Ok(());
            }
            Err(e) if e == err::CANCELLED => {
                session.send_cancel("user").await;
                return Err(e);
            }
            Err(e) if e == svc::PEER_CANCEL_MARK => {
                // 对端取消：永久终止，**不进续传窗口**
                return Err(e);
            }
            Err(e) if !e.starts_with(err::TRANSFER_FAILED) => {
                // 协议/语义级失败（对端拒绝、磁盘满、完整性不符、源文件不可读…）：
                // 重连也不会变好，直接终态（真机实测：完整性失败曾触发无限续传循环）
                return Err(e);
            }
            Err(e) => {
                // 断线 / IO 错误 → 窗口内重连（resume 语义在 Initial 帧）
                log::warn!(
                    "lan_file: stream error ({e}), attempt={} resume window",
                    attempt
                );
                attempt += 1;
                if total_elapsed_exceeded(attempt) {
                    return Err(e);
                }
                // 退避期间 resuming 反馈（契约 F4）
                emit_transfer(
                    &app,
                    &progress_snapshot(
                        &transfer_id,
                        "send",
                        &peer_id,
                        &terminal_name,
                        &metas,
                        transferred,
                        total,
                        0.0,
                        TransferState::Resuming,
                        None,
                    ),
                );
                if wait_with_cancel(&cmd_rx, retry_delay(attempt - 1)).await {
                    return Err(err::CANCELLED.into());
                }
                continue;
            }
        }
    }
}

/// 退避等待：分片睡眠并轮询取消；返回 true = 收到取消。
async fn wait_with_cancel(cmd_rx: &Receiver<TaskCommand>, wait: Duration) -> bool {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if let Ok(TaskCommand::Cancel) = cmd_rx.try_recv() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        let remain = deadline.saturating_duration_since(std::time::Instant::now());
        tokio::time::sleep(remain.min(Duration::from_millis(100))).await;
    }
}

/// 退避累计是否超出续传窗口（120s）。
fn total_elapsed_exceeded(attempt: u32) -> bool {
    let total: u64 = svc::RETRY_BACKOFF_SECS.iter().take(attempt as usize).sum();
    total >= RESUME_WINDOW.as_secs()
}

#[allow(clippy::too_many_arguments)]
async fn send_file_stream(
    app: &AppHandle,
    session: &mut SecureSession,
    cmd_rx: &std::sync::mpsc::Receiver<TaskCommand>,
    file_index: u32,
    path: &str,
    meta: &OfferFileInfo,
    skip: u64,
    rate: &mut RateEstimator,
    last_emit: &mut std::time::Instant,
    transfer_id: &str,
    peer_id: &str,
    terminal_name: &str,
    metas: &[OfferFileInfo],
    transferred: &mut [u64],
    total: u64,
) -> Result<(), String> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            err::FILE_NOT_FOUND.to_string()
        } else {
            err::FILE_UNREADABLE.to_string()
        }
    })?;
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(skip))
        .map_err(|_| err::FILE_UNREADABLE.to_string())?;

    let mut sent = skip;
    // seq = 该字节位置所在的块号（续传时从同号块继续，与接收方期望的 AAD 一致）
    let mut seq = svc::chunk_seq_at(skip);
    let mut buf = vec![0u8; CHUNK_SIZE];
    rate.reset(sent, now_ms());
    while sent < meta.size {
        // 取消侦测（非阻塞轮询）
        if let Ok(TaskCommand::Cancel) = cmd_rx.try_recv() {
            return Err(err::CANCELLED.into());
        }
        let want = buf.len().min((meta.size - sent) as usize);
        let read_t0 = std::time::Instant::now();
        let n = file
            .read(&mut buf[..want])
            .map_err(|_| err::FILE_UNREADABLE.to_string())?;
        if n == 0 {
            return Err(err::FILE_UNREADABLE.into());
        }
        if read_t0.elapsed() > Duration::from_secs(1) {
            log::warn!(
                "lan_file: disk read slow ({}ms) at {sent}B of {}",
                read_t0.elapsed().as_millis(),
                meta.size
            );
        }
        // 写块：带**停滞判定**——零进展 30s 说明链路已塌（丢包使 cwnd 塌到 2 段、
        // RTT 秒级），此时按断线处理（重连 + 断点续传）远比干等快。
        let write_t0 = std::time::Instant::now();
        match tokio::time::timeout(
            svc::CHUNK_STALL_TIMEOUT,
            session.send_chunk(file_index, seq, &buf[..n]),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(session_err_to_task(e)),
            Err(_) => {
                log::warn!(
                    "lan_file: send stalled >{}s at {sent}B (peer/link zero progress), reconnect+resume",
                    svc::CHUNK_STALL_TIMEOUT.as_secs()
                );
                return Err(format!(
                    "{}: send stalled >{}s at {sent}B",
                    err::TRANSFER_FAILED,
                    svc::CHUNK_STALL_TIMEOUT.as_secs()
                ));
            }
        }
        if write_t0.elapsed() > Duration::from_secs(2) {
            log::warn!(
                "lan_file: chunk write slow ({}ms) at {sent}B",
                write_t0.elapsed().as_millis()
            );
        }
        sent += n as u64;
        seq += 1;
        transferred[file_index as usize] = sent;
        // 进度节流推送
        if last_emit.elapsed() >= PROGRESS_INTERVAL {
            *last_emit = std::time::Instant::now();
            let bps = rate.observe(total_progress(metas, transferred), now_ms());
            emit_transfer(
                app,
                &progress_snapshot(
                    transfer_id,
                    "send",
                    peer_id,
                    terminal_name,
                    metas,
                    transferred,
                    total,
                    bps,
                    TransferState::Transferring,
                    None,
                ),
            );
        }
    }

    // 本文件哈希 → End
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    {
        let mut f2 = std::fs::File::open(path).map_err(|_| err::FILE_UNREADABLE.to_string())?;
        std::io::copy(&mut f2, &mut hasher).map_err(|_| err::FILE_UNREADABLE.to_string())?;
    }
    let hex = super::store::hex_encode(&hasher.finalize());
    session
        .send_json(&EndFrame { sha256: vec![hex] })
        .await
        .map_err(session_err_to_task)?;
    // Ack（对账结论）
    let ack: AckFrame = session.recv_json().await.map_err(session_err_to_task)?;
    if !ack.ok {
        return Err(ack.code.unwrap_or_else(|| err::INTEGRITY_MISMATCH.into()));
    }
    let _ = transfer_id;
    Ok(())
}

fn total_progress(metas: &[OfferFileInfo], transferred: &[u64]) -> u64 {
    metas
        .iter()
        .enumerate()
        .map(|(i, m)| transferred.get(i).copied().unwrap_or(0).min(m.size))
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn progress_snapshot(
    transfer_id: &str,
    direction: &str,
    peer_id: &str,
    terminal_name: &str,
    metas: &[OfferFileInfo],
    transferred: &[u64],
    total: u64,
    bps: f64,
    state: TransferState,
    error: Option<TransferError>,
) -> LanFileTransfer {
    LanFileTransfer {
        transfer_id: Some(transfer_id.to_string()),
        direction: direction.to_string(),
        peer_id: Some(peer_id.to_string()),
        terminal_name: Some(terminal_name.to_string()),
        state,
        files: metas
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let done = transferred.get(i).copied().unwrap_or(0);
                TransferFileInfo {
                    name: m.name.clone(),
                    size: m.size,
                    transferred_bytes: done.min(m.size),
                    status: if done >= m.size {
                        FileStatus::Done
                    } else if done > 0 {
                        FileStatus::Transferring
                    } else {
                        FileStatus::Pending
                    },
                }
            })
            .collect(),
        total_bytes: total,
        transferred_bytes: total_progress(metas, transferred).min(total),
        bytes_per_sec: bps,
        saved_paths: None,
        error,
    }
}

/// 等待应答帧：同时轮询任务命令（取消）与提议窗口超时。
async fn wait_reply(
    session: &mut SecureSession,
    cmd_rx: &std::sync::mpsc::Receiver<TaskCommand>,
    timeout: Duration,
) -> Result<ReplyFrame, WaitError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(TaskCommand::Cancel) = cmd_rx.try_recv() {
            return Err(WaitError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(WaitError::Timeout);
        }
        // 收帧（短超时分片等待，兼顾取消轮询）；
        // 注：帧读取本身是取消安全的（`read_frame_buffered`），分片取消不会破坏帧流。
        match tokio::time::timeout(
            Duration::from_millis(200),
            session.recv_json::<ReplyFrame>(),
        )
        .await
        {
            Ok(Ok(reply)) => return Ok(reply),
            Ok(Err(SessionError::Cancelled(_))) => return Err(WaitError::PeerCancelled),
            Ok(Err(e)) => return Err(WaitError::Session(e)),
            Err(_) => continue, // 分片超时 → 继续轮询
        }
    }
}

enum WaitError {
    Cancelled,
    PeerCancelled,
    Timeout,
    Session(SessionError),
}

/// 会话错误 → 任务错误串（对端取消用内部标记；其余带诊断细节）。
fn session_err_to_task(e: SessionError) -> String {
    match e {
        SessionError::Cancelled(_reason) => svc::PEER_CANCEL_MARK.to_string(),
        other => format!("{}: {other}", err::TRANSFER_FAILED),
    }
}

// ---------------------------------------------------------------------------
// 接收方：交互传输（Offer / ResumeOffer）
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn receive_interactive(
    app: AppHandle,
    mut session: SecureSession,
    peer: PeerIdentity,
    transfer_id: String,
    files: Vec<OfferFile>,
    total_bytes: u64,
    is_resume: bool,
) -> Result<(), SessionError> {
    let peer_id = peer.peer_id.clone();
    let fingerprint = peer.fingerprint.clone();
    let terminal_name = peer_terminal(&peer_id);

    // 续传判定（契约 5.5）：sidecar 存在 && 无墓碑 && 窗口未过 → Accept{resume}
    let data_dir = app.path().app_data_dir().unwrap_or_default();
    let inbox_dir = data_dir.join("lanfile");
    let sidecar = if is_resume {
        load_sidecar(&inbox_dir, &transfer_id).unwrap_or(None)
    } else {
        None
    };

    // 第二个新提议（非续传）→ 自动拒绝 busy（契约 5.3：本端 notify warning）
    if !is_resume {
        let occupied = shared()
            .map(|g| g.lock().unwrap().active_transfer.is_some())
            .unwrap_or(false);
        if occupied {
            let _ = session
                .send_json(&ReplyFrame::Reject {
                    code: err::BUSY.into(),
                })
                .await;
            session.shutdown().await;
            log::info!("lan_file: second offer {transfer_id} auto-rejected (busy)");
            notify_app(&app, NotifyLevel::Warning, err::BUSY);
            return Ok(());
        }
    }

    // 磁盘空间预检（总大小 + 200MB 余量，契约 5.5）
    if !check_disk_space(&inbox_dir, total_bytes) {
        let _ = session
            .send_json(&ReplyFrame::Reject {
                code: err::DISK_FULL.into(),
            })
            .await;
        session.shutdown().await;
        return Ok(());
    }

    // 会话占用（接收方也在交互会话槽）；续传允许顶掉同一 transferId 的陈旧会话
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<TaskCommand>();
    let cmd_rx = Arc::new(std::sync::Mutex::new(cmd_rx));
    let Some(epoch) = try_occupy_session(
        &transfer_id,
        "receive",
        &peer_id,
        &terminal_name,
        cmd_tx.clone(),
        is_resume,
    ) else {
        let _ = session
            .send_json(&ReplyFrame::Reject {
                code: err::BUSY.into(),
            })
            .await;
        session.shutdown().await;
        return Ok(());
    };

    // 续传合法性
    let resume_offsets: Option<Vec<u64>> = sidecar.as_ref().and_then(|meta| {
        if meta.is_cancelled() {
            None
        } else {
            Some(meta.files.iter().map(|f| f.received_bytes).collect())
        }
    });
    if is_resume && sidecar.is_none() {
        // 无 sidecar 的续传请求 → 拒绝（对端将任务失败）
        release_session(&transfer_id, epoch);
        let _ = session
            .send_json(&ReplyFrame::Reject {
                code: err::TRANSFER_FAILED.into(),
            })
            .await;
        session.shutdown().await;
        return Ok(());
    }
    if let Some(meta) = sidecar.as_ref() {
        if meta.is_cancelled() {
            // 墓碑：取消 = 永久终止，不再续传（契约 5.5）
            release_session(&transfer_id, epoch);
            let _ = session
                .send_json(&ReplyFrame::Reject {
                    code: err::CANCELLED.into(),
                })
                .await;
            session.shutdown().await;
            return Ok(());
        }
    }

    // 提议到达（契约 5.4）：仅**陌生**终端需要人工确认；已信任终端免确认直接进入传输。
    // 关键接线：陌生提议必须 `register_pending_offer`，否则 accept/reject 命令查表落空
    // （历史 bug：面板点「接受」立刻报 lan_file.peer_not_found）。
    if !is_resume {
        let (name_clash, known_minutes, trusted) = {
            let shared = shared();
            match shared {
                Some(shared) => {
                    let g = shared.lock().unwrap();
                    (
                        g.settings.name_clash(&peer_id, &terminal_name),
                        g.announces
                            .iter()
                            .find(|e| e.announce.peer_id == peer_id)
                            .map(|e| now_ms().saturating_sub(e.last_seen_ms) / 60_000)
                            .unwrap_or(0),
                        g.settings.is_trusted(&peer_id),
                    )
                }
                None => (false, 0, false),
            }
        };
        if !trusted {
            let offer = LanFileOffer {
                transfer_id: transfer_id.clone(),
                peer_id: peer_id.clone(),
                terminal_name: terminal_name.clone(),
                fingerprint,
                name_clash,
                files: files
                    .iter()
                    .map(|f| OfferFileInfo {
                        name: sanitize_filename(&f.name),
                        size: f.size,
                    })
                    .collect(),
                total_bytes,
                known_from_minutes: known_minutes,
            };
            // 先注册待决提议，再 emit 事件（避免前端点击快于注册的竞态）
            register_pending_offer(&transfer_id, &peer_id, &terminal_name, cmd_tx.clone());
            let _ = app.emit(INCOMING_EVENT, &offer);
            // spawn_blocking：同步轮询用户决定（&Receiver 非 Sync 不能跨 .await）
            let rx = Arc::clone(&cmd_rx);
            let outcome = tokio::task::spawn_blocking(move || {
                let rx = rx.lock().unwrap();
                wait_user_decision_blocking(&rx, OFFER_TIMEOUT)
            })
            .await
            .unwrap_or(UserDecision::Timeout);
            unregister_pending_offer(&transfer_id);
            match outcome {
                UserDecision::Accept => {
                    // TOFU：写入信任表（契约 5.4；命令层亦已写，此处兜底幂等）
                    let _ = trust_peer(&app, &peer_id, &terminal_name);
                }
                UserDecision::Reject => {
                    release_session(&transfer_id, epoch);
                    let _ = session
                        .send_json(&ReplyFrame::Reject {
                            code: err::REJECTED.into(),
                        })
                        .await;
                    session.shutdown().await;
                    emit_terminal(
                        &app,
                        TerminalSnapshot {
                            transfer_id: &transfer_id,
                            direction: "receive",
                            peer_id: &peer_id,
                            terminal_name: &terminal_name,
                            state: TransferState::Rejected,
                            files: recv_files_snapshot(&files, &vec![0; files.len()]),
                            total_bytes,
                            transferred_bytes: 0,
                            saved_paths: None,
                            error: Some(err::REJECTED.into()),
                        },
                    );
                    notify_app(&app, NotifyLevel::Info, err::REJECTED);
                    return Ok(());
                }
                UserDecision::Timeout => {
                    release_session(&transfer_id, epoch);
                    let _ = session
                        .send_json(&ReplyFrame::Reject {
                            code: err::OFFER_TIMEOUT.into(),
                        })
                        .await;
                    session.shutdown().await;
                    emit_terminal(
                        &app,
                        TerminalSnapshot {
                            transfer_id: &transfer_id,
                            direction: "receive",
                            peer_id: &peer_id,
                            terminal_name: &terminal_name,
                            state: TransferState::Failed,
                            files: recv_files_snapshot(&files, &vec![0; files.len()]),
                            total_bytes,
                            transferred_bytes: 0,
                            saved_paths: None,
                            error: Some(err::OFFER_TIMEOUT.into()),
                        },
                    );
                    notify_app(&app, NotifyLevel::Warning, err::OFFER_TIMEOUT);
                    return Ok(());
                }
                UserDecision::Cancel => {
                    user_cancel_receive(&app, &inbox_dir, &transfer_id, &peer_id, &files);
                    release_session(&transfer_id, epoch);
                    let _ = session.send_cancel("user").await;
                    emit_terminal(
                        &app,
                        TerminalSnapshot {
                            transfer_id: &transfer_id,
                            direction: "receive",
                            peer_id: &peer_id,
                            terminal_name: &terminal_name,
                            state: TransferState::Cancelled,
                            files: recv_files_snapshot(&files, &vec![0; files.len()]),
                            total_bytes,
                            transferred_bytes: 0,
                            saved_paths: None,
                            error: Some(err::CANCELLED.into()),
                        },
                    );
                    notify_app(&app, NotifyLevel::Info, err::CANCELLED);
                    return Ok(());
                }
            }
        } else {
            log::debug!("lan_file: offer {transfer_id} from trusted peer {peer_id} auto-accepted");
        }
    } else if sidecar.is_none() {
        // 理论不可达（前面已拒）
        release_session(&transfer_id, epoch);
        session.shutdown().await;
        return Ok(());
    }

    // 发 Accept（fresh 或 resume offsets）
    let reply = match &resume_offsets {
        Some(offsets) => ReplyFrame::Accept {
            fresh: false,
            per_file_received_bytes: Some(offsets.clone()),
        },
        None => ReplyFrame::Accept {
            fresh: true,
            per_file_received_bytes: None,
        },
    };
    if let Err(e) = session.send_json(&reply).await {
        release_session(&transfer_id, epoch);
        return Err(e);
    }
    // 接受后立即推 transferring（小文件可能不足一个进度节流周期，否则前端只见终态）
    emit_transfer(
        &app,
        &recv_state_snapshot(
            &transfer_id,
            &peer_id,
            &terminal_name,
            &files,
            &resume_offsets
                .clone()
                .unwrap_or_else(|| vec![0; files.len()]),
            total_bytes,
            TransferState::Transferring,
            None,
        ),
    );

    // 准备落盘：净化文件名 + 重名去重 + sidecar
    let mut final_names: Vec<String> = Vec::with_capacity(files.len());
    {
        let existing: Vec<String> = std::fs::read_dir(&inbox_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_file())
                    .filter_map(|e| e.file_name().to_string_lossy().into_owned().into())
                    .collect()
            })
            .unwrap_or_default();
        let mut planned: Vec<String> = existing.clone();
        for f in &files {
            let clean = sanitize_filename(&f.name);
            let final_name = dedup_filename(&clean, &|n| planned.contains(&n.to_string()));
            planned.push(final_name.clone());
            final_names.push(final_name);
        }
    }
    let _ = std::fs::create_dir_all(&inbox_dir);

    // sidecar 初始化（接收侧续传依据）
    let mut sidecar_meta = SidecarMeta {
        transfer_id: transfer_id.clone(),
        direction: "receive".into(),
        peer_id: peer_id.clone(),
        files: files
            .iter()
            .enumerate()
            .map(|(i, f)| SidecarFile {
                name: f.name.clone(),
                tmp_name: tmp_name_for(&transfer_id, i),
                received_bytes: resume_offsets
                    .as_ref()
                    .and_then(|o| o.get(i).copied())
                    .unwrap_or(0),
            })
            .collect(),
        cancelled: None,
    };
    let _ = save_sidecar(&inbox_dir, &sidecar_meta);

    // 逐文件接收（串行；契约 5.3 多文件串行）
    //
    // `transferred` 必须以**续传偏移**为初值：否则续传会话把 `start` 当成 0，
    // 对已有 `.tmp` 再次 append 整份文件（真机实测：50MB 文件涨到 55MB + 哈希对账必失败
    // → 发送方误判可重试 → 无限续传循环）。
    let mut transferred: Vec<u64> = resume_offsets
        .clone()
        .unwrap_or_else(|| vec![0; files.len()]);
    let mut saved_paths: Vec<String> = Vec::with_capacity(files.len());
    let mut rate = RateEstimator::new();
    let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
    let mut received_hashes: Vec<String> = Vec::with_capacity(files.len());
    let mut failed: Option<String> = None;
    // 本地发起取消（用户点「取消传输」/ 关开关）——终态不显示「对方取消了传输」
    let mut local_cancel = false;
    'outer: for (idx, file) in files.iter().enumerate() {
        let tmp_path = inbox_dir.join(tmp_name_for(&transfer_id, idx));
        let start = transferred[idx];
        // 续传对齐（契约 5.5）：sidecar 每 250ms 落一次，而 `.tmp` 是连续写入的——
        // 异常中断时 `.tmp` 往往比 sidecar 记录的偏移**更长**，直接 append 会整体错位
        // （真机实测：文件 100% 传完却哈希对账失败、被丢弃）。故先按记录偏移截断。
        if start > 0 {
            let actual = std::fs::metadata(&tmp_path).map(|m| m.len()).unwrap_or(0);
            if actual < start {
                // 记录偏移 > 实际文件（数据丢失）→ 清理并失败，不再续传
                log::warn!("lan_file: resume offset {start} > tmp size {actual}, discard partial");
                remove_tmp_file(&inbox_dir, &tmp_name_for(&transfer_id, idx));
                let _ = remove_sidecar(&inbox_dir, &transfer_id);
                failed = Some(err::INTEGRITY_MISMATCH.into());
                break 'outer;
            }
            if actual > start {
                match std::fs::OpenOptions::new().write(true).open(&tmp_path) {
                    Ok(f) => {
                        if f.set_len(start).is_err() {
                            failed = Some(err::STORAGE_ERROR.into());
                            break 'outer;
                        }
                    }
                    Err(e) => {
                        failed = Some(format!("{}: {e}", err::STORAGE_ERROR));
                        break 'outer;
                    }
                }
            }
        }
        let mut out = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&tmp_path)
        {
            Ok(f) => f,
            Err(e) => {
                failed = Some(format!("{}: {e}", err::STORAGE_ERROR));
                break;
            }
        };
        let mut hasher = sha2::Sha256::new();
        // 续传：已收部分流式进哈希（大文件不能整块读进内存）
        if start > 0 {
            match std::fs::File::open(&tmp_path) {
                Ok(mut f2) => {
                    if std::io::copy(&mut f2, &mut hasher).is_err() {
                        failed = Some(err::STORAGE_ERROR.into());
                        break 'outer;
                    }
                }
                Err(e) => {
                    failed = Some(format!("{}: {e}", err::STORAGE_ERROR));
                    break 'outer;
                }
            }
        }
        let mut got = start;
        // seq = 该字节位置所在的块号（与发送方 seek 后从同号块继续一致，AAD 绑定位置）
        let mut seq = svc::chunk_seq_at(start);
        while got < file.size {
            // 取消侦测（cmd_rx 为 Arc<Mutex<Receiver>>）
            if let Ok(TaskCommand::Cancel) = cmd_rx.lock().unwrap().try_recv() {
                failed = Some(err::CANCELLED.into());
                local_cancel = true;
                break 'outer;
            }
            // 收 Chunk（AAD 绑定 fileIndex+seq）——分片等待并轮询取消：
            // 数据面无帧级硬超时，对端静默时 recv 会一直阻塞，若只在块间轮询取消，
            // 用户点「取消传输」将毫无反应（真机实测）。帧读取本身取消安全。
            let wait_t0 = std::time::Instant::now();
            let chunk = loop {
                if let Ok(TaskCommand::Cancel) = cmd_rx.lock().unwrap().try_recv() {
                    failed = Some(err::CANCELLED.into());
                    local_cancel = true;
                    break 'outer;
                }
                // 彻底收不到数据（远超发送侧停滞阈值）→ 判链路已死：
                // 释放会话槽并保留 sidecar，让对端重连能立刻被接受（否则会被拒成 busy）。
                if wait_t0.elapsed() > svc::RECV_DEAD_TIMEOUT {
                    log::warn!(
                        "lan_file: no data for {}s at {got}B/{} → treat link dead, wait for resume",
                        wait_t0.elapsed().as_secs(),
                        file.size
                    );
                    break Err(SessionError::Io("recv idle timeout".into()));
                }
                match tokio::time::timeout(
                    Duration::from_millis(200),
                    session.recv_chunk(idx as u32, seq),
                )
                .await
                {
                    Ok(r) => break r,
                    Err(_) => continue, // 分片超时 → 继续轮询取消
                }
            };
            // 卡顿定位：单块等待超阈值只记日志（不中断传输，链路慢但仍有进展）
            if wait_t0.elapsed() > svc::RECV_STALL_LOG_THRESHOLD {
                log::warn!(
                    "lan_file: recv stall {}ms at {got}B/{} (peer link slow)",
                    wait_t0.elapsed().as_millis(),
                    file.size
                );
            }
            let chunk = match chunk {
                Ok(c) => c,
                Err(SessionError::Cancelled(_reason)) => {
                    // 对端显式取消（契约 5.5）：墓碑 + 删 .tmp，终态 cancelled，**不进续传窗口**
                    use std::io::Write as _;
                    let _ = out.flush();
                    drop(out);
                    user_cancel_receive(&app, &inbox_dir, &transfer_id, &peer_id, &files);
                    release_session(&transfer_id, epoch);
                    emit_terminal(
                        &app,
                        TerminalSnapshot {
                            transfer_id: &transfer_id,
                            direction: "receive",
                            peer_id: &peer_id,
                            terminal_name: &terminal_name,
                            state: TransferState::Cancelled,
                            files: recv_files_snapshot(&files, &transferred),
                            total_bytes,
                            transferred_bytes: total_bytes_recv(&files, &transferred),
                            saved_paths: None,
                            error: Some(err::CANCELLED.into()),
                        },
                    );
                    notify_app(&app, NotifyLevel::Info, err::CANCELLED);
                    return Ok(());
                }
                Err(_e) => {
                    // 异常中断：sidecar 保留进 120s 窗口（契约 5.5）——
                    // 前端转 resuming 琥珀横幅；窗口内对端重连即续传，超窗由
                    // `spawn_resume_expiry` 清理并落 failed(transfer_failed)。
                    use std::io::Write as _;
                    let _ = out.flush();
                    sidecar_meta.files[idx].received_bytes = got;
                    let _ = save_sidecar(&inbox_dir, &sidecar_meta);
                    release_session(&transfer_id, epoch);
                    emit_transfer(
                        &app,
                        &recv_state_snapshot(
                            &transfer_id,
                            &peer_id,
                            &terminal_name,
                            &files,
                            &transferred,
                            total_bytes,
                            TransferState::Resuming,
                            None,
                        ),
                    );
                    spawn_resume_expiry(app.clone(), inbox_dir.clone(), transfer_id.clone());
                    return Ok(()); // 任务级失败已通知；会话错误不回传
                }
            };
            use std::io::Write as _;
            let write_t0 = std::time::Instant::now();
            if out.write_all(&chunk).is_err() {
                failed = Some(err::STORAGE_ERROR.into());
                break 'outer;
            }
            if write_t0.elapsed() > Duration::from_secs(1) {
                log::warn!(
                    "lan_file: disk write slow ({}ms) at {got}B (av scan / disk busy?)",
                    write_t0.elapsed().as_millis()
                );
            }
            use sha2::Digest as _;
            hasher.update(&chunk);
            got += chunk.len() as u64;
            seq += 1;
            transferred[idx] = got;
            sidecar_meta.files[idx].received_bytes = got;
            // 进度节流推送 + sidecar 刷新（低频）
            if last_emit.elapsed() >= PROGRESS_INTERVAL {
                last_emit = std::time::Instant::now();
                let _ = save_sidecar(&inbox_dir, &sidecar_meta);
                let bps = rate.observe(total_bytes_recv(&files, &transferred), now_ms());
                emit_transfer(
                    &app,
                    &recv_progress_snapshot(
                        &transfer_id,
                        &peer_id,
                        &terminal_name,
                        &files,
                        &transferred,
                        total_bytes,
                        bps,
                        None,
                    ),
                );
            }
        }
        use std::io::Write as _;
        let _ = out.flush();
        drop(out);

        // End 帧对账（全文件重哈希）
        let end: EndFrame = match session.recv_json().await {
            Ok(e) => e,
            Err(SessionError::Cancelled(_)) => {
                failed = Some(err::CANCELLED.into());
                break;
            }
            Err(e) => {
                failed = Some(format!("{}: {e}", err::TRANSFER_FAILED));
                break;
            }
        };
        use sha2::Digest as _;
        let local_hex = super::store::hex_encode(&hasher.finalize());
        if end.sha256.first().map(|h| h.as_str()) != Some(local_hex.as_str()) {
            // 完整性失败：删全部 .tmp，任务失败（契约 5.5）
            for i in 0..files.len() {
                remove_tmp_file(&inbox_dir, &tmp_name_for(&transfer_id, i));
            }
            let _ = session
                .send_json(&AckFrame {
                    ok: false,
                    code: Some(err::INTEGRITY_MISMATCH.into()),
                })
                .await;
            release_session(&transfer_id, epoch);
            emit_terminal(
                &app,
                TerminalSnapshot {
                    transfer_id: &transfer_id,
                    direction: "receive",
                    peer_id: &peer_id,
                    terminal_name: &terminal_name,
                    state: TransferState::Failed,
                    files: recv_files_snapshot(&files, &transferred),
                    total_bytes,
                    transferred_bytes: total_bytes_recv(&files, &transferred),
                    saved_paths: None,
                    error: Some(err::INTEGRITY_MISMATCH.into()),
                },
            );
            notify_app(&app, NotifyLevel::Error, err::INTEGRITY_MISMATCH);
            return Ok(());
        }
        received_hashes.push(local_hex);
        if let Err(e) = session
            .send_json(&AckFrame {
                ok: true,
                code: None,
            })
            .await
        {
            failed = Some(format!("{}: {e}", err::TRANSFER_FAILED));
            break;
        }
        // 原子 rename 出正式名（契约 5.5）
        let final_path = inbox_dir.join(&final_names[idx]);
        let _ = std::fs::rename(&tmp_path, &final_path);
        saved_paths.push(final_path.to_string_lossy().into_owned());
        sidecar_meta.files[idx].received_bytes = file.size;
        let _ = save_sidecar(&inbox_dir, &sidecar_meta);
    }

    // 任务收尾
    release_session(&transfer_id, epoch);
    if failed.is_some() && failed.as_deref() == Some(err::CANCELLED) {
        // 用户取消：墓碑 + 删 tmp（契约 5.5）
        user_cancel_receive(&app, &inbox_dir, &transfer_id, &peer_id, &files);
        session.send_cancel("user").await;
        emit_terminal(
            &app,
            TerminalSnapshot {
                transfer_id: &transfer_id,
                direction: "receive",
                peer_id: &peer_id,
                terminal_name: &terminal_name,
                state: TransferState::Cancelled,
                files: recv_files_snapshot(&files, &transferred),
                total_bytes,
                transferred_bytes: total_bytes_recv(&files, &transferred),
                saved_paths: None,
                error: if local_cancel {
                    None
                } else {
                    Some(err::CANCELLED.into())
                },
            },
        );
        notify_app(&app, NotifyLevel::Info, err::CANCELLED);
        return Ok(());
    }
    if let Some(err_code) = failed {
        emit_terminal(
            &app,
            TerminalSnapshot {
                transfer_id: &transfer_id,
                direction: "receive",
                peer_id: &peer_id,
                terminal_name: &terminal_name,
                state: TransferState::Failed,
                files: recv_files_snapshot(&files, &transferred),
                total_bytes,
                transferred_bytes: total_bytes_recv(&files, &transferred),
                saved_paths: None,
                error: Some(err_code.clone()),
            },
        );
        notify_app(&app, NotifyLevel::Warning, &err_code);
        return Ok(());
    }

    // done：清 sidecar，通知 + 快照（停留展示，含「打开所在位置」）
    let _ = remove_sidecar(&inbox_dir, &transfer_id);
    emit_terminal(
        &app,
        TerminalSnapshot {
            transfer_id: &transfer_id,
            direction: "receive",
            peer_id: &peer_id,
            terminal_name: &terminal_name,
            state: TransferState::Done,
            files: recv_files_snapshot(&files, &transferred),
            total_bytes,
            transferred_bytes: total_bytes,
            saved_paths: Some(saved_paths.clone()),
            error: None,
        },
    );
    notify_app(&app, NotifyLevel::Success, "lan_file.done");
    Ok(())
}

enum UserDecision {
    Accept,
    Reject,
    Timeout,
    Cancel,
}

/// 等待用户决定（同步阻塞轮询；调用方经 tokio::task::spawn_blocking 进入，
/// 避免 `&Receiver`（非 Sync）跨 .await 破坏 Send）。
fn wait_user_decision_blocking(
    cmd_rx: &std::sync::mpsc::Receiver<TaskCommand>,
    timeout: Duration,
) -> UserDecision {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match cmd_rx.try_recv() {
            Ok(TaskCommand::Accept) => return UserDecision::Accept,
            Ok(TaskCommand::Reject) => return UserDecision::Reject,
            Ok(TaskCommand::Cancel) => return UserDecision::Cancel,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return UserDecision::Cancel,
        }
        if std::time::Instant::now() >= deadline {
            return UserDecision::Timeout;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 用户取消（接收侧）：写墓碑 + 删 tmp（保留墓碑至窗口自然过期，契约 5.5）。
fn user_cancel_receive(
    app: &AppHandle,
    inbox_dir: &Path,
    transfer_id: &str,
    _peer_id: &str,
    files: &[OfferFile],
) {
    let _ = app;
    if let Some(mut meta) = load_sidecar(inbox_dir, transfer_id).unwrap_or(None) {
        meta.cancelled = Some(true);
        let _ = save_sidecar(inbox_dir, &meta);
    } else {
        let meta = SidecarMeta {
            transfer_id: transfer_id.to_string(),
            direction: "receive".into(),
            peer_id: _peer_id.to_string(),
            files: files
                .iter()
                .enumerate()
                .map(|(i, f)| SidecarFile {
                    name: f.name.clone(),
                    tmp_name: tmp_name_for(transfer_id, i),
                    received_bytes: 0,
                })
                .collect(),
            cancelled: Some(true),
        };
        let _ = save_sidecar(inbox_dir, &meta);
    }
    for i in 0..files.len() {
        remove_tmp_file(inbox_dir, &tmp_name_for(transfer_id, i));
    }
}

fn tmp_name_for(transfer_id: &str, index: usize) -> String {
    format!(".{transfer_id}.{index}.tmp")
}

fn total_bytes_recv(files: &[OfferFile], transferred: &[u64]) -> u64 {
    files
        .iter()
        .enumerate()
        .map(|(i, f)| transferred.get(i).copied().unwrap_or(0).min(f.size))
        .sum()
}

fn recv_files_snapshot(files: &[OfferFile], transferred: &[u64]) -> Vec<TransferFileInfo> {
    files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let done = transferred.get(i).copied().unwrap_or(0);
            TransferFileInfo {
                name: sanitize_filename(&f.name),
                size: f.size,
                transferred_bytes: done.min(f.size),
                status: if done >= f.size {
                    FileStatus::Done
                } else if done > 0 {
                    FileStatus::Transferring
                } else {
                    FileStatus::Pending
                },
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn recv_progress_snapshot(
    transfer_id: &str,
    peer_id: &str,
    terminal_name: &str,
    files: &[OfferFile],
    transferred: &[u64],
    total: u64,
    bps: f64,
    error: Option<TransferError>,
) -> LanFileTransfer {
    let mut snap = recv_state_snapshot(
        transfer_id,
        peer_id,
        terminal_name,
        files,
        transferred,
        total,
        TransferState::Transferring,
        error,
    );
    snap.bytes_per_sec = bps;
    snap
}

/// 接收侧任意状态快照（transferring / resuming）。
#[allow(clippy::too_many_arguments)]
fn recv_state_snapshot(
    transfer_id: &str,
    peer_id: &str,
    terminal_name: &str,
    files: &[OfferFile],
    transferred: &[u64],
    total: u64,
    state: TransferState,
    error: Option<TransferError>,
) -> LanFileTransfer {
    LanFileTransfer {
        transfer_id: Some(transfer_id.to_string()),
        direction: "receive".into(),
        peer_id: Some(peer_id.to_string()),
        terminal_name: Some(terminal_name.to_string()),
        state,
        files: recv_files_snapshot(files, transferred),
        total_bytes: total,
        transferred_bytes: total_bytes_recv(files, transferred).min(total),
        bytes_per_sec: 0.0,
        saved_paths: None,
        error,
    }
}

/// 续传窗口到期检查（契约 5.5「超窗无重连 → 删除 sidecar 与 .tmp，
/// 任务 failed(transfer_failed) + notify warning」）。
///
/// 独立线程 + sleep 实现（无帧级硬超时要求，窗口固定 120s）：
/// 到期时若该 transferId 既无 sidecar 也不再占用会话槽 → 视为超窗清理；
/// 对端已在窗口内重连（sidecar 仍存在但会话重新占用）则留给进行中的任务处理。
fn spawn_resume_expiry(app: AppHandle, inbox_dir: PathBuf, transfer_id: String) {
    std::thread::Builder::new()
        .name("lan-file-resume-expiry".into())
        .spawn(move || {
            std::thread::sleep(RESUME_WINDOW);
            let still_active = shared()
                .map(|g| {
                    let g = g.lock().unwrap();
                    g.active_transfer
                        .as_ref()
                        .map(|t| t.transfer_id == transfer_id)
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if still_active {
                return; // 窗口内已重连续传
            }
            let meta = match load_sidecar(&inbox_dir, &transfer_id) {
                Ok(Some(m)) => m,
                Ok(None) => return, // 已完成 / 已取消清理
                Err(_) => return,
            };
            if meta.is_cancelled() {
                // 墓碑：保留至自然过期即可（下次启动清理），不重复通知
                return;
            }
            for f in &meta.files {
                remove_tmp_file(&inbox_dir, &f.tmp_name);
            }
            let _ = remove_sidecar(&inbox_dir, &transfer_id);
            let files: Vec<TransferFileInfo> = meta
                .files
                .iter()
                .map(|f| TransferFileInfo {
                    name: sanitize_filename(&f.name),
                    size: 0,
                    transferred_bytes: f.received_bytes,
                    status: FileStatus::Transferring,
                })
                .collect();
            let transferred: u64 = meta.files.iter().map(|f| f.received_bytes).sum();
            emit_terminal(
                &app,
                TerminalSnapshot {
                    transfer_id: &transfer_id,
                    direction: "receive",
                    peer_id: &meta.peer_id,
                    terminal_name: &peer_terminal(&meta.peer_id),
                    state: TransferState::Failed,
                    files,
                    total_bytes: transferred,
                    transferred_bytes: transferred,
                    saved_paths: None,
                    error: Some(err::TRANSFER_FAILED.into()),
                },
            );
            notify_app(&app, NotifyLevel::Warning, err::TRANSFER_FAILED);
            log::info!("lan_file: resume window expired for {transfer_id}, cleaned up");
        })
        .ok();
}

/// 对端终端名（公告快照；无公告时用 peerId 短号）。
fn peer_terminal(peer_id: &str) -> String {
    shared()
        .and_then(|g| {
            let g = g.lock().unwrap();
            g.announces
                .iter()
                .find(|e| e.announce.peer_id == peer_id)
                .map(|e| e.announce.terminal.clone())
        })
        .unwrap_or_else(|| format!("peer-{}", &peer_id[..peer_id.len().min(8)]))
}

/// 磁盘空间检查（fs4 查剩余；检查失败按通过处理——落盘错误另行报）。
fn check_disk_space(dir: &Path, total_bytes: u64) -> bool {
    std::fs::create_dir_all(dir).ok();
    match fs4::available_space(dir) {
        Ok(avail) => disk_space_ok(avail, total_bytes),
        Err(e) => {
            log::warn!("lan_file: disk space check failed: {e}");
            true
        }
    }
}

// ---------------------------------------------------------------------------
// peerId → LAN IP 学习表（公告不带 IP；VLF 独立 TCP 需要对端地址）
// ---------------------------------------------------------------------------

static PEER_IPS: OnceLock<Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();

fn peer_ips() -> &'static Mutex<std::collections::HashMap<String, String>> {
    PEER_IPS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 学习：peerId 最近一次已知地址（入站连接 remote / 出站成功 dial）。
pub fn learn_peer_addr(peer_id: &str, addr: &str) {
    let mut map = peer_ips().lock().unwrap();
    map.insert(peer_id.to_string(), addr.to_string());
}

/// 查询对端最近已知地址。
pub fn peer_addr(peer_id: &str) -> Option<String> {
    peer_ips().lock().unwrap().get(peer_id).cloned()
}

/// 解析对端 dial 地址（学习到的 IP + 当前公告端口）。
///
/// 续传重连时**必须**重新解析：接收方进程重启后动态端口会变（契约 5.2）。
fn resolve_peer_addr(peer_id: &str) -> Option<String> {
    let ip = peer_addr(peer_id)?;
    let port = {
        let shared = shared()?;
        let g = shared.lock().unwrap();
        find_announce(&g.announces, peer_id).map(|a| a.tcp_port)?
    };
    Some(format!("{ip}:{port}"))
}

// ---------------------------------------------------------------------------
// 剪贴板主题新鲜观察表（移动端图片通道免人工信任的第二层证据，契约 5.9）
// ---------------------------------------------------------------------------

/// peerId → 最近一次在剪贴板主题上出现的时刻（unix 毫秒）。
static CLIPBOARD_SEEN: OnceLock<Mutex<std::collections::HashMap<String, u64>>> = OnceLock::new();

fn clipboard_seen() -> &'static Mutex<std::collections::HashMap<String, u64>> {
    CLIPBOARD_SEEN.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 记录「该 peerId 刚在剪贴板主题上出现过」（lan-sync 收到信封时调用）。
pub fn note_clipboard_peer(peer_id: &str) {
    let mut map = clipboard_seen().lock().unwrap();
    map.insert(peer_id.to_string(), now_ms());
    // 容量卫生：长期运行只留新鲜项（5 分钟窗口外的记录无意义）
    let cutoff = now_ms().saturating_sub(svc::CLIPBOARD_FRESH_WINDOW.as_millis() as u64);
    map.retain(|_, seen| *seen >= cutoff);
}

/// 该 peerId 是否仍在剪贴板新鲜窗口内（契约 5.9：5 分钟）。
pub fn clipboard_peer_fresh(peer_id: &str) -> bool {
    let map = clipboard_seen().lock().unwrap();
    map.get(peer_id)
        .map(|seen| svc::clipboard_peer_fresh(*seen, now_ms()))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// 自动图片通道（契约 5.7）
// ---------------------------------------------------------------------------

/// 图片自动通道发送计划（发送侧门槛全部通过时的产物）。
///
/// 契约 5.7-1：信封必须先带上 `imageMeta.hash`/`xfer: true` 再广播，否则接收侧
/// 收件箱条目没有关联键、字节到了也点不亮（历史 bug：hash 永不回填）。
#[derive(Debug, Clone)]
pub struct ImageXferPlan {
    pub path: PathBuf,
    pub name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: u64,
    /// 图片字节 SHA-256 hex（信封 hash 与 ImageOffer hash 共用同一值）。
    pub hash: String,
    /// 目标终端（在线且 caps 含 img）。
    pub targets: Vec<Announce>,
}

/// capture 钩子入口（发送侧门槛判定，契约 5.7-5/6）：返回计划表示「信封要带 hash/xfer
/// 且字节将经通道送达」；返回 None = 本次不走通道（信封不带 hash/xfer，收件箱维持占位）。
///
/// 门槛：本功能开 + lan-sync 广播开 + 白名单扩展名 + ≤10MiB + 可读可哈希 + 至少一个
/// 在线 caps(img) 终端。任何失败静默（无事件、无错误 UI，仅日志）。
pub fn image_xfer_plan(entry: &serde_json::Value) -> Option<ImageXferPlan> {
    if !lan_file_enabled() {
        return None;
    }
    // lan-sync 广播开关关闭 → 不发（契约 5.7-5）
    if !hooks::lan_sync_broadcast_enabled().unwrap_or(false) {
        return None;
    }
    let image = entry.get("image")?;
    let path = image.get("path").and_then(|v| v.as_str())?;
    let size = image.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let width = image
        .get("width")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let height = image
        .get("height")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    if !is_image_ext(&name) {
        log::debug!("lan_file: image channel skip (ext not whitelisted): {name}");
        return None;
    }
    if size > 10 * 1024 * 1024 {
        log::info!("lan_file: image channel skip (>10MiB): {name} {size}B");
        return None;
    }
    // hash：读文件算 SHA-256（发送侧关联键）
    let hash = match super::store::file_sha256_hex(Path::new(path)) {
        Ok(h) => h,
        Err(e) => {
            log::debug!("lan_file: image channel skip (unreadable): {e}");
            return None;
        }
    };
    let targets: Vec<Announce> = {
        let shared = shared()?;
        let g = shared.lock().unwrap();
        g.announces
            .iter()
            .filter(|e| e.announce.supports_image() && e.announce.peer_id != g.self_peer_id)
            .map(|e| e.announce.clone())
            .collect()
    };
    if targets.is_empty() {
        log::debug!("lan_file: image channel no online img-capable peers");
        return None;
    }
    Some(ImageXferPlan {
        path: PathBuf::from(path),
        name,
        width,
        height,
        size,
        hash,
        targets,
    })
}

/// 按计划入队（契约 5.7-6：对每个在线 caps(img) 终端各入队一次，串行小队列）。
///
/// 调用时机：lan-sync 已把 `hash`/`xfer` 写入信封并完成广播之后。
pub fn queue_image_offers(app: &AppHandle, plan: ImageXferPlan) {
    let Some(shared) = shared() else { return };
    let mut g = shared.lock().unwrap();
    for target in &plan.targets {
        let job = ImageOfferJob {
            transfer_id: uuid::Uuid::new_v4().to_string(),
            path: plan.path.clone(),
            name: plan.name.clone(),
            width: plan.width,
            height: plan.height,
            size: plan.size,
            hash: plan.hash.clone(),
            target: target.clone(),
        };
        g.image_queue.push_back(job);
        // 队列深度上限：超出丢最旧（截图小文件，丢弃无损，契约 5.3）
        while g.image_queue.len() > IMAGE_QUEUE_CAP {
            if let Some(dropped) = g.image_queue.pop_front() {
                log::info!(
                    "lan_file: image queue overflow, dropped oldest {}",
                    dropped.name
                );
            }
        }
    }
    drop(g);
    // 唤醒 worker（通道信号，worker 常驻轮询亦可）
    if let Some(tx) = IMAGE_WAKE.get() {
        let _ = tx.send(());
    }
    let _ = app;
}

static IMAGE_WAKE: OnceLock<std::sync::mpsc::Sender<()>> = OnceLock::new();

/// 图片 worker：串行处理队列（每项 = 对一个目标终端的完整 VLF 会话）。
fn start_image_worker(app: AppHandle, signing: ed25519_dalek::SigningKey) {
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
    let _ = IMAGE_WAKE.set(wake_tx);
    let self_peer_id = shared()
        .map(|g| g.lock().unwrap().self_peer_id.clone())
        .unwrap_or_default();
    std::thread::Builder::new()
        .name("lan-file-image".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("lan-file-image-rt")
                .build();
            let Ok(rt) = rt else {
                log::error!("lan_file: image worker runtime build failed");
                return;
            };
            rt.block_on(image_worker_loop(app, signing, self_peer_id, wake_rx));
        })
        .ok();
}

async fn image_worker_loop(
    app: AppHandle,
    signing: ed25519_dalek::SigningKey,
    self_peer_id: String,
    wake_rx: Receiver<()>,
) {
    loop {
        if SHUTTING_DOWN.load(Ordering::SeqCst) {
            break;
        }
        // 取队首（串行）
        let job = {
            let Some(shared) = shared() else { break };
            let mut g = shared.lock().unwrap();
            g.image_queue.pop_front()
        };
        match job {
            Some(job) => {
                if let Err(e) = send_image(&app, &signing, &self_peer_id, &job).await {
                    log::info!(
                        "lan_file: image offer to {} failed (silent): {e}",
                        job.target.peer_id
                    );
                }
            }
            None => {
                // 空闲：等唤醒或 500ms 轮询
                match wake_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }
    }
    log::debug!("lan_file: image worker stopped");
}

/// 发送一张图（独立 VLF 会话；静默语义，不占交互会话槽）。
async fn send_image(
    app: &AppHandle,
    signing: &ed25519_dalek::SigningKey,
    self_peer_id: &str,
    job: &ImageOfferJob,
) -> Result<(), String> {
    let addr = peer_addr(&job.target.peer_id)
        .map(|ip| format!("{}:{}", ip, job.target.tcp_port))
        .ok_or_else(|| "no known addr".to_string())?;
    let (mut session, _peer) = SecureSession::connect(&addr, signing, self_peer_id)
        .await
        .map_err(|e| e.to_string())?;
    session
        .send_json(&InitialFrame::ImageOffer {
            transfer_id: job.transfer_id.clone(),
            name: job.name.clone(),
            width: job.width,
            height: job.height,
            size: job.size,
            hash: job.hash.clone(),
        })
        .await
        .map_err(|e| e.to_string())?;
    let reply: ReplyFrame = session.recv_json().await.map_err(|e| e.to_string())?;
    match reply {
        ReplyFrame::Reject { code } => {
            return Err(format!("rejected: {code}"));
        }
        ReplyFrame::Accept { fresh, .. } => {
            if !fresh {
                return Err("unexpected resume accept".into());
            }
        }
    }
    // 分帧发送
    let data = std::fs::read(&job.path).map_err(|e| format!("read image: {e}"))?;
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest as _;
    hasher.update(&data);
    let mut sent = 0usize;
    let mut seq = 0u32;
    while sent < data.len() {
        let end = (sent + CHUNK_SIZE).min(data.len());
        session
            .send_chunk(0, seq, &data[sent..end])
            .await
            .map_err(|e| e.to_string())?;
        sent = end;
        seq += 1;
    }
    session
        .send_json(&EndFrame {
            sha256: vec![super::store::hex_encode(&hasher.finalize())],
        })
        .await
        .map_err(|e| e.to_string())?;
    let ack: AckFrame = session.recv_json().await.map_err(|e| e.to_string())?;
    if !ack.ok {
        return Err("image integrity mismatch".into());
    }
    session.shutdown().await;
    log::info!(
        "lan_file: image sent {} ({}B) to {}",
        job.name,
        job.size,
        job.target.peer_id
    );
    let _ = app;
    Ok(())
}

/// 接收一张图（门控 → 落盘 → LRU；全静默，契约 5.7）。
///
/// 门控（桌面，5.7-2，全部满足才落盘）：
/// 1. 来源 peerId ∈ 本端信任表；2. hash 匹配（ImageOffer 与信封一致）；
/// 3. 字节 ≤ 10MiB；4. 扩展名白名单；5. 磁盘检查。
#[allow(clippy::too_many_arguments)]
async fn receive_image(
    app: AppHandle,
    mut session: SecureSession,
    peer: PeerIdentity,
    transfer_id: String,
    name: String,
    // width/height 预留（落盘文件名按 hash，元数据已在信封内）
    _width: Option<u32>,
    _height: Option<u32>,
    size: u64,
    hash: String,
) -> Result<(), SessionError> {
    let peer_id = peer.peer_id.clone();
    async fn reject_session(session: &mut SecureSession, code: &str) {
        let _ = session
            .send_json(&ReplyFrame::Reject {
                code: code.to_string(),
            })
            .await;
        session.shutdown().await;
    }

    // 收图方向开关（契约 5.7-5）：本功能关 / lan-sync 接收关 → 不落盘
    let enabled = lan_file_enabled();
    let sync_receive = hooks::lan_sync_receive_enabled().unwrap_or(false);
    let trusted = shared()
        .map(|g| g.lock().unwrap().settings.is_trusted(&peer_id))
        .unwrap_or(false);
    // 来源门控（契约 5.7-2 桌面 / 5.9 移动端）：桌面认信任表；移动端无信任 UI，
    // 改由「VLF 握手已认证 peerId（multihash(公钥)==peerId）」+「5 分钟剪贴板新鲜观察」放行。
    let source_ok =
        svc::image_source_admissible(trusted, clipboard_peer_fresh(&peer_id), cfg!(mobile));
    let admissible = size <= 10 * 1024 * 1024 && is_image_ext(&name);
    if !enabled || !sync_receive || !source_ok || !admissible {
        let code = if !source_ok {
            "lan_file.not_trusted"
        } else {
            err::NOT_ENABLED
        };
        log::debug!(
            "lan_file: image from {peer_id} rejected silently (enabled={enabled} sync_recv={sync_receive} source_ok={source_ok} trusted={trusted} admissible={admissible})"
        );
        reject_session(&mut session, code).await;
        return Ok(());
    }
    let _ = transfer_id;

    // 磁盘检查
    let data_dir = app.path().app_data_dir().unwrap_or_default();
    let img_dir = data_dir.join("lan-inbox-images");
    if !check_disk_space(&img_dir, size) {
        log::info!("lan_file: image {name} rejected (disk full), silent");
        reject_session(&mut session, err::DISK_FULL).await;
        return Ok(());
    }

    // 收数据
    session
        .send_json(&ReplyFrame::Accept {
            fresh: true,
            per_file_received_bytes: None,
        })
        .await?;
    let _ = std::fs::create_dir_all(&img_dir);
    let ext = Path::new(&name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "png".into());
    let final_path = img_dir.join(format!("{hash}.{ext}"));
    let tmp_path = img_dir.join(format!(".img-{hash}.tmp"));
    let mut out = std::fs::File::create(&tmp_path).map_err(|e| SessionError::Io(e.to_string()))?;
    let mut hasher = sha2::Sha256::new();
    let mut got = 0u64;
    let mut seq = 0u32;
    while got < size {
        let chunk = session.recv_chunk(0, seq).await?;
        use std::io::Write as _;
        out.write_all(&chunk)
            .map_err(|e| SessionError::Io(e.to_string()))?;
        use sha2::Digest as _;
        hasher.update(&chunk);
        got += chunk.len() as u64;
        seq += 1;
    }
    drop(out);
    // 哈希对账（对账失败 → 静默丢弃）
    use sha2::Digest as _;
    let local_hex = super::store::hex_encode(&hasher.finalize());
    if local_hex != hash {
        let _ = std::fs::remove_file(&tmp_path);
        let _ = session
            .send_json(&AckFrame {
                ok: false,
                code: Some(err::INTEGRITY_MISMATCH.into()),
            })
            .await;
        session.shutdown().await;
        log::info!("lan_file: image {name} hash mismatch, discarded silently");
        return Ok(());
    }
    let _ = std::fs::rename(&tmp_path, &final_path);
    let _ = session
        .send_json(&AckFrame {
            ok: true,
            code: None,
        })
        .await;
    session.shutdown().await;

    // LRU 清理（200 张 / 500MB，超限删最旧，契约 5.7-3）
    lru_cleanup(&img_dir);

    // 收件箱点亮经既有 lan-sync://inbox-updated 路径：图片条目按 hash 命中即可，
    // 此处 emit 一次 inbox-updated 促使前端重查（lan-sync 契约 5.7-4）
    let _ = app.emit(
        crate::features::lan_sync::state::INBOX_UPDATED_EVENT,
        serde_json::json!({ "reason": "image-arrived" }),
    );
    log::info!("lan_file: image {name} ({}B) saved from {peer_id}", size);
    Ok(())
}

/// 图片目录 LRU 清理。
fn lru_cleanup(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, SystemTime, u64)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.path(), meta.modified().ok()?, meta.len()))
        })
        .collect();
    let evict = lru_evict_images(&files);
    for p in evict {
        let _ = std::fs::remove_file(&p);
        log::debug!("lan_file: LRU evicted {}", p.display());
    }
    files.clear();
}
