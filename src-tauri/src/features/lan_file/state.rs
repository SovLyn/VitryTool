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
}

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

    // 终端名与 lan-sync 同源（设置页一处改名，处处生效）
    let terminal_name = crate::features::lan_sync::state::shared()
        .map(|g| g.lock().unwrap().settings.terminal_name.clone())
        .unwrap_or_else(|| "VitryTool".into());

    let fingerprint = super::transport::crypto::fingerprint_of(&signing.verifying_key());

    // 启动 TCP 监听（0.0.0.0:0 动态端口，契约 5.2）
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
            let port = rt.block_on(run_listener(listener_app, listen_signing, self_pid));
            let _ = listen_tx.send(port);
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
    let _ = app.emit(
        SETTINGS_UPDATED_EVENT,
        serde_json::json!({ "fileShare": enabled }),
    );
    log::info!("lan_file: enabled={enabled}");
    Ok(())
}

/// 发布本机公告（启动 / 周期 / 发现新对端后补发）。
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
    };
    let Ok(bytes) = serde_json::to_vec(&announce) else {
        return;
    };
    let _ = enabled; // 公告照常发布（关闭仅停数据面 + 拒收），对端展示用 caps 判断
    let state = app.state::<AppState>();
    let node = state.peer_node.lock().unwrap();
    if let Some(node) = node.as_ref() {
        node.publish(ANNOUNCE_TOPIC, bytes);
        log::debug!("lan_file: announce published (port={port})");
    } else {
        log::warn!("lan_file: peer_node not running, announce skipped");
    }
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
    {
        let Some(shared) = shared() else { return };
        let mut g = shared.lock().unwrap();
        if announce.peer_id == g.self_peer_id {
            return; // 自己的公告
        }
        upsert_announce(&mut g.announces, announce, now_ms());
        changed = true;
    }
    if changed {
        let _ = app.emit(PEERS_UPDATED_EVENT, ());
        // 发现新对端后补公告（等 gossipsub 订阅握手，复用 0.2.5 经验）：
        // 简化为每次收到公告都补发一次自身公告（低频，5min TTL 内最多对端数 × 1）
        publish_announce(app);
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

/// 与新 peer 建立连接（PeerConnected 事件）：立即补发公告 + 2s 延迟再补一次
/// （第一次可能早于对端 gossipsub 订阅握手完成被丢弃，契约 5.2「发现新对端后补公告」；
/// 对端收到后也会补发，形成交叉回响，消灭最长 5min 的发现盲区）。
pub fn on_peer_connected(app: &AppHandle) {
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
) -> Option<u16> {
    let listener = match tokio::net::TcpListener::bind("0.0.0.0:0").await {
        Ok(l) => l,
        Err(e) => {
            log::error!("lan_file: tcp listen failed: {e}");
            return None;
        }
    };
    let port = listener.local_addr().ok().map(|a| a.port());
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
    port
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

/// 取活跃任务槽（互斥占用，契约 5.3 单会话）。
fn try_occupy_session(
    transfer_id: &str,
    direction: &'static str,
    peer_id: &str,
    terminal_name: &str,
    cmd_tx: Sender<TaskCommand>,
) -> bool {
    let Some(shared) = shared() else { return false };
    let mut g = shared.lock().unwrap();
    if g.active_transfer.is_some() {
        return false;
    }
    g.active_transfer = Some(ActiveTask {
        transfer_id: transfer_id.to_string(),
        direction,
        peer_id: peer_id.to_string(),
        terminal_name: terminal_name.to_string(),
        cmd_tx,
    });
    true
}

fn release_session(transfer_id: &str) {
    if let Some(shared) = shared() {
        let mut g = shared.lock().unwrap();
        if g.active_transfer
            .as_ref()
            .map(|t| t.transfer_id == transfer_id)
            .unwrap_or(false)
        {
            g.active_transfer = None;
        }
    }
}

/// 通知前端活跃任务结束（idle 快照）。
fn emit_idle(app: &AppHandle, direction: &str) {
    emit_transfer(app, &idle_snapshot(direction));
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

/// 用户取消：定向活跃任务（发送侧 Cancel 帧 / 接收侧墓碑由任务内处理）。
pub fn cancel_transfer(transfer_id: &str) {
    // 待决提议取消
    resolve_offer(transfer_id, TaskCommand::Cancel);
    // 活跃任务取消
    let tx = {
        let Some(shared) = shared() else { return };
        let g = shared.lock().unwrap();
        g.active_transfer
            .as_ref()
            .filter(|t| t.transfer_id == transfer_id)
            .map(|t| t.cmd_tx.clone())
    };
    if let Some(tx) = tx {
        let _ = tx.send(TaskCommand::Cancel);
    }
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
    if !try_occupy_session(
        &transfer_id,
        "send",
        &peer_id,
        &terminal_name,
        cmd_tx.clone(),
    ) {
        return Err(err::BUSY.into());
    }

    let addr = {
        let Some(shared) = shared() else {
            return Err("lan-file not initialized".into());
        };
        let g = shared.lock().unwrap();
        find_announce(&g.announces, &peer_id)
            .map(|a| format!("{}:{}", a_ip(a), a.tcp_port))
            .ok_or_else(|| err::PEER_NOT_FOUND.to_string())?
    };

    let result = send_task_inner(
        app.clone(),
        signing,
        self_peer_id,
        addr,
        paths,
        metas,
        total,
        transfer_id.clone(),
        cmd_rx,
    )
    .await;

    release_session(&transfer_id);
    emit_idle(&app, "send");
    match &result {
        Ok(_) => {
            notify_app(&app, NotifyLevel::Success, "lan_file.done");
        }
        Err(e) if e.contains(err::CANCELLED) => {
            notify_app(&app, NotifyLevel::Info, err::CANCELLED);
        }
        Err(e) => {
            log::warn!("lan_file: send failed: {e}");
            notify_app(&app, NotifyLevel::Warning, err::TRANSFER_FAILED);
        }
    }
    let _ = peer_trusted;
    result
}

/// 公告中的 IP 来源不可从公告负载取（gossipsub 无 IP 字段）——
/// dial 地址用 mDNS 已建立连接的可达性不可行（VLF 独立 TCP），
/// 实现为：公告携带本机最近与该 peer 通信所见的 socket 地址不可靠，
/// v1 直接用「对端公告里没有 IP」的现实 → 采用 mdns 发现时记录的地址表
/// （peer_node 在 Discovered 时 dial，连接建立后其 socket remote 即对端 LAN IP）。
/// 简化实现：维护 peerId → 最近已知 IP 表，由入站连接与公告触发点学习。
fn a_ip(_a: &Announce) -> String {
    String::new()
}

#[allow(clippy::too_many_arguments)]
async fn send_task_inner(
    app: AppHandle,
    signing: ed25519_dalek::SigningKey,
    self_peer_id: String,
    addr: String,
    paths: Vec<String>,
    metas: Vec<OfferFileInfo>,
    total: u64,
    transfer_id: String,
    cmd_rx: Receiver<TaskCommand>,
) -> Result<Vec<String>, String> {
    let mut attempt: u32 = 0u32;
    let saved: Vec<String> = Vec::new();
    let mut transferred: Vec<u64> = vec![0; metas.len()];

    // 任务快照推送（节流 ≤4/s）
    let mut rate = RateEstimator::new();
    let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);

    loop {
        // 占位：重连循环在 dial 失败路径处理
        let connect = SecureSession::connect(&addr, &signing, &self_peer_id).await;
        let (mut session, _peer) = match connect {
            Ok(v) => v,
            Err(SessionError::Io(_e)) => {
                // dial 失败：窗口内退避重试
                if attempt >= svc::RETRY_BACKOFF_SECS.len() as u32
                    || total_elapsed_exceeded(attempt)
                {
                    return Err(err::TRANSFER_FAILED.into());
                }
                tokio::time::sleep(retry_delay(attempt)).await;
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

        // 命令轮询（cancel）+ 应答等待（60s 提议窗口）
        let reply = wait_reply(&mut session, &cmd_rx, OFFER_TIMEOUT).await;
        let reply = match reply {
            Ok(r) => r,
            Err(WaitError::Cancelled) => {
                // 用户取消：发 Cancel 帧 + 本地无墓碑（发送侧无落盘）
                session.send_cancel("user").await;
                return Err(err::CANCELLED.into());
            }
            Err(WaitError::Timeout) => {
                session.send_cancel("offer_timeout").await;
                return Err(err::OFFER_TIMEOUT.into());
            }
            Err(WaitError::Session(e)) => return Err(format!("{}: {e}", err::TRANSFER_FAILED)),
        };
        match reply {
            ReplyFrame::Reject { code } => {
                // 对端拒绝：磁盘满 / 忙 / 已取消墓碑等（契约 5.5）
                return Err(code);
            }
            ReplyFrame::Accept {
                fresh,
                per_file_received_bytes,
            } => {
                if !fresh {
                    if let Some(offsets) = per_file_received_bytes {
                        transferred = offsets;
                    }
                }
            }
        }

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
                &metas,
                &mut transferred,
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
                return Ok(saved);
            }
            Err(e) if e == err::CANCELLED => {
                session.send_cancel("user").await;
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
                tokio::time::sleep(retry_delay(attempt - 1)).await;
                continue;
            }
        }
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
    let mut seq = 0u32;
    let mut buf = vec![0u8; CHUNK_SIZE];
    rate.reset(sent, now_ms());
    while sent < meta.size {
        // 取消侦测（非阻塞轮询）
        if let Ok(TaskCommand::Cancel) = cmd_rx.try_recv() {
            return Err(err::CANCELLED.into());
        }
        let want = buf.len().min((meta.size - sent) as usize);
        let n = file
            .read(&mut buf[..want])
            .map_err(|_| err::FILE_UNREADABLE.to_string())?;
        if n == 0 {
            return Err(err::FILE_UNREADABLE.into());
        }
        session
            .send_chunk(file_index, seq, &buf[..n])
            .await
            .map_err(|e| e.to_string())?;
        sent += n as u64;
        seq += 1;
        transferred[file_index as usize] = sent;
        // 进度节流推送
        if last_emit.elapsed() >= PROGRESS_INTERVAL {
            *last_emit = std::time::Instant::now();
            let bps = rate.observe(total_progress(metas, transferred), now_ms());
            emit_transfer(
                app,
                &progress_snapshot(transfer_id, "send", metas, transferred, total, bps, None),
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
        .map_err(|e| e.to_string())?;
    // Ack（对账结论）
    let ack: AckFrame = session.recv_json().await.map_err(|e| e.to_string())?;
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
    metas: &[OfferFileInfo],
    transferred: &[u64],
    total: u64,
    bps: f64,
    error: Option<TransferError>,
) -> LanFileTransfer {
    LanFileTransfer {
        transfer_id: Some(transfer_id.to_string()),
        direction: direction.to_string(),
        peer_id: None,
        terminal_name: None,
        state: TransferState::Transferring,
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
        // 收帧（短超时分片等待，兼顾取消轮询）
        match tokio::time::timeout(
            Duration::from_millis(200),
            session.recv_json::<ReplyFrame>(),
        )
        .await
        {
            Ok(Ok(reply)) => return Ok(reply),
            Ok(Err(e)) => return Err(WaitError::Session(e)),
            Err(_) => continue, // 分片超时 → 继续轮询
        }
    }
}

enum WaitError {
    Cancelled,
    Timeout,
    Session(SessionError),
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

    // 第二个新提议（非续传）→ 自动拒绝 busy（契约 5.3）
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

    // 会话占用（接收方也在交互会话槽）
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<TaskCommand>();
    let cmd_rx = Arc::new(std::sync::Mutex::new(cmd_rx));
    if !try_occupy_session(&transfer_id, "receive", &peer_id, &terminal_name, cmd_tx) {
        let _ = session
            .send_json(&ReplyFrame::Reject {
                code: err::BUSY.into(),
            })
            .await;
        session.shutdown().await;
        return Ok(());
    }

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
        release_session(&transfer_id);
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
            release_session(&transfer_id);
            let _ = session
                .send_json(&ReplyFrame::Reject {
                    code: err::CANCELLED.into(),
                })
                .await;
            session.shutdown().await;
            return Ok(());
        }
    }

    // emitting offer 事件（resumed 不发，契约 5.4/5.5）
    if !is_resume {
        let (name_clash, known_minutes) = {
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
                    )
                }
                None => (false, 0),
            }
        };
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
        let _ = app.emit(INCOMING_EVENT, &offer);
        // 信任表内 → 自动放行（TOFU 免确认）；陌生 → 等用户 accept/reject（60s 超时自动拒）
        let trusted = shared()
            .map(|g| g.lock().unwrap().settings.is_trusted(&peer_id))
            .unwrap_or(false);
        if !trusted {
            // spawn_blocking：同步轮询用户决定（&Receiver 非 Sync 不能跨 .await）
            let rx = Arc::clone(&cmd_rx);
            let outcome = tokio::task::spawn_blocking(move || {
                let rx = rx.lock().unwrap();
                wait_user_decision_blocking(&rx, OFFER_TIMEOUT)
            })
            .await
            .unwrap_or(UserDecision::Timeout);
            match outcome {
                UserDecision::Accept => {
                    // TOFU：写入信任表（契约 5.4）
                    let _ = trust_peer(&app, &peer_id, &terminal_name);
                }
                UserDecision::Reject => {
                    release_session(&transfer_id);
                    let _ = session
                        .send_json(&ReplyFrame::Reject {
                            code: err::REJECTED.into(),
                        })
                        .await;
                    session.shutdown().await;
                    emit_idle(&app, "receive");
                    return Ok(());
                }
                UserDecision::Timeout => {
                    release_session(&transfer_id);
                    let _ = session
                        .send_json(&ReplyFrame::Reject {
                            code: err::OFFER_TIMEOUT.into(),
                        })
                        .await;
                    session.shutdown().await;
                    emit_idle(&app, "receive");
                    return Ok(());
                }
                UserDecision::Cancel => {
                    user_cancel_receive(&app, &inbox_dir, &transfer_id, &peer_id, &files);
                    release_session(&transfer_id);
                    let _ = session.send_cancel("user").await;
                    emit_idle(&app, "receive");
                    notify_app(&app, NotifyLevel::Info, err::CANCELLED);
                    return Ok(());
                }
            }
        }
    } else if sidecar.is_none() {
        // 理论不可达（前面已拒）
        release_session(&transfer_id);
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
        release_session(&transfer_id);
        return Err(e);
    }

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
    let mut transferred: Vec<u64> = vec![0; files.len()];
    let mut saved_paths: Vec<String> = Vec::with_capacity(files.len());
    let mut rate = RateEstimator::new();
    let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
    let mut received_hashes: Vec<String> = Vec::with_capacity(files.len());
    let mut failed: Option<String> = None;
    'outer: for (idx, file) in files.iter().enumerate() {
        let tmp_path = inbox_dir.join(tmp_name_for(&transfer_id, idx));
        let start = transferred[idx];
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
        // 续传：已收部分进哈希
        if start > 0 {
            if let Ok(bytes) = std::fs::read(&tmp_path) {
                use sha2::Digest as _;
                hasher.update(&bytes);
            }
        }
        let mut got = start;
        let mut seq = (start / CHUNK_SIZE as u64) as u32;
        while got < file.size {
            // 取消侦测（cmd_rx 为 Arc<Mutex<Receiver>>）
            if let Ok(TaskCommand::Cancel) = cmd_rx.lock().unwrap().try_recv() {
                failed = Some(err::CANCELLED.into());
                break 'outer;
            }
            // 收 Chunk（AAD 绑定 fileIndex+seq）
            let chunk = match session.recv_chunk(idx as u32, seq).await {
                Ok(c) => c,
                Err(_e) => {
                    // 异常中断：sidecar 保留进 120s 窗口（契约 5.5）
                    // 注：failed 在此路径不消费——直接走「窗口保留 + 静默等重连」出口
                    use std::io::Write as _;
                    let _ = out.flush();
                    sidecar_meta.files[idx].received_bytes = got;
                    let _ = save_sidecar(&inbox_dir, &sidecar_meta);
                    release_session(&transfer_id);
                    notify_app(&app, NotifyLevel::Warning, err::TRANSFER_FAILED);
                    emit_idle(&app, "receive");
                    return Ok(()); // 任务级失败已通知；会话错误不回传
                }
            };
            use std::io::Write as _;
            if out.write_all(&chunk).is_err() {
                failed = Some(err::STORAGE_ERROR.into());
                break 'outer;
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
            release_session(&transfer_id);
            emit_transfer(
                &app,
                &LanFileTransfer {
                    transfer_id: Some(transfer_id.clone()),
                    direction: "receive".into(),
                    peer_id: Some(peer_id.clone()),
                    terminal_name: Some(terminal_name.clone()),
                    state: TransferState::Failed,
                    files: recv_files_snapshot(&files, &transferred),
                    total_bytes,
                    transferred_bytes: total_bytes_recv(&files, &transferred),
                    bytes_per_sec: 0.0,
                    saved_paths: None,
                    error: Some(TransferError {
                        code: err::INTEGRITY_MISMATCH.into(),
                        params: None,
                    }),
                },
            );
            notify_app(&app, NotifyLevel::Error, err::INTEGRITY_MISMATCH);
            emit_idle(&app, "receive");
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
    release_session(&transfer_id);
    if failed.is_some() && failed.as_deref() == Some(err::CANCELLED) {
        // 用户取消：墓碑 + 删 tmp（契约 5.5）
        user_cancel_receive(&app, &inbox_dir, &transfer_id, &peer_id, &files);
        session.send_cancel("user").await;
        emit_idle(&app, "receive");
        notify_app(&app, NotifyLevel::Info, err::CANCELLED);
        return Ok(());
    }
    if let Some(err_code) = failed {
        emit_transfer(
            &app,
            &LanFileTransfer {
                transfer_id: Some(transfer_id.clone()),
                direction: "receive".into(),
                peer_id: Some(peer_id.clone()),
                terminal_name: Some(terminal_name.clone()),
                state: TransferState::Failed,
                files: recv_files_snapshot(&files, &transferred),
                total_bytes,
                transferred_bytes: total_bytes_recv(&files, &transferred),
                bytes_per_sec: 0.0,
                saved_paths: None,
                error: Some(TransferError {
                    code: err_code.clone(),
                    params: None,
                }),
            },
        );
        notify_app(&app, NotifyLevel::Warning, &err_code);
        emit_idle(&app, "receive");
        return Ok(());
    }

    // done：清 sidecar，通知 + 快照
    let _ = remove_sidecar(&inbox_dir, &transfer_id);
    emit_transfer(
        &app,
        &LanFileTransfer {
            transfer_id: Some(transfer_id.clone()),
            direction: "receive".into(),
            peer_id: Some(peer_id.clone()),
            terminal_name: Some(terminal_name.clone()),
            state: TransferState::Done,
            files: recv_files_snapshot(&files, &transferred),
            total_bytes,
            transferred_bytes: total_bytes,
            bytes_per_sec: 0.0,
            saved_paths: Some(saved_paths.clone()),
            error: None,
        },
    );
    notify_app(&app, NotifyLevel::Success, "lan_file.done");
    emit_idle(&app, "receive");
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
    LanFileTransfer {
        transfer_id: Some(transfer_id.to_string()),
        direction: "receive".into(),
        peer_id: Some(peer_id.to_string()),
        terminal_name: Some(terminal_name.to_string()),
        state: TransferState::Transferring,
        files: recv_files_snapshot(files, transferred),
        total_bytes: total,
        transferred_bytes: total_bytes_recv(files, transferred).min(total),
        bytes_per_sec: bps,
        saved_paths: None,
        error,
    }
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

// ---------------------------------------------------------------------------
// 自动图片通道（契约 5.7）
// ---------------------------------------------------------------------------

/// capture 钩子入口：新图片条目 → 门槛过滤 → 对每个在线 caps(img) 终端入队。
///
/// 发送侧先卡 10MiB + 白名单（复审修订 D①：不排队注定被拒的传输）；
/// 任何失败静默（无事件、无错误 UI，仅日志）。
pub fn queue_image_offers(app: &AppHandle, entry: &serde_json::Value) {
    // 前置开关：本功能关闭 → 不发（信封元数据广播照常，归 lan_sync）
    if !lan_file_enabled() {
        return;
    }
    // lan-sync 广播开关关闭 → 不发（契约 5.7-5）
    if !hooks::lan_sync_broadcast_enabled().unwrap_or(false) {
        return;
    }
    let Some(image) = entry.get("image") else {
        return;
    };
    let Some(path) = image.get("path").and_then(|v| v.as_str()) else {
        return;
    };
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
        return;
    }
    if size > 10 * 1024 * 1024 {
        log::info!("lan_file: image channel skip (>10MiB): {name} {size}B");
        return;
    }
    // hash：直接读文件算 SHA-256（发送侧关联键）
    let hash = match super::store::file_sha256_hex(Path::new(path)) {
        Ok(h) => h,
        Err(e) => {
            log::debug!("lan_file: image channel skip (unreadable): {e}");
            return;
        }
    };

    // 回填 imageMeta.hash/xfer 到广播信封由 lan_sync 在构造时查询（见 lan_sync::state）。
    // 此处：对每个在线 caps(img) 终端入队 ImageOffer
    let targets: Vec<Announce> = {
        let Some(shared) = shared() else { return };
        let g = shared.lock().unwrap();
        g.announces
            .iter()
            .filter(|e| e.announce.supports_image() && e.announce.peer_id != g.self_peer_id)
            .map(|e| e.announce.clone())
            .collect()
    };
    if targets.is_empty() {
        log::debug!("lan_file: image channel no online img-capable peers");
        return;
    }
    let Some(shared) = shared() else { return };
    let mut g = shared.lock().unwrap();
    for target in targets {
        let job = ImageOfferJob {
            transfer_id: uuid::Uuid::new_v4().to_string(),
            path: PathBuf::from(path),
            name: name.clone(),
            width,
            height,
            size,
            hash: hash.clone(),
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
    let admissible = size <= 10 * 1024 * 1024 && is_image_ext(&name);
    if !enabled || !sync_receive || !trusted || !admissible {
        let code = if !trusted {
            "lan_file.not_trusted"
        } else {
            err::NOT_ENABLED
        };
        log::debug!(
            "lan_file: image from {peer_id} rejected silently (enabled={enabled} sync_recv={sync_receive} trusted={trusted} admissible={admissible})"
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
