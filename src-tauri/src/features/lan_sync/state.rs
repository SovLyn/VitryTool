//! lan-sync 运行时共享状态与后台线程（节点事件消费 + 广播钩子）。
//!
//! 设计（契约 `docs/api/lan-sync.md` 5.1-5.4）：
//! - 共享态（收件箱 / 设置 / 防环指纹 / 自身身份）以 `OnceLock<Arc<Mutex<_>>>` 持有，
//!   命令与后台线程共享；setup 时初始化（`init`）。
//! - 消费者线程：从节点事件通道接收 pubsub 消息 → 解析信封 → 入收件箱 → 持久化 → 通知前端。
//! - 广播钩子：经 `core::hooks` 接收剪贴板历史「新条目」→ 防环检查 → 构建信封 → 节点发布。

use super::service::{
    envelope_fingerprint, inbox_entry_from_envelope, insert_message, Envelope, InboxData,
    InboxOutcome, LanSettings, MAX_MESSAGE_BYTES, MAX_RECEIVED_FINGERPRINTS,
};
use super::store::{InboxStore, SettingsStore, StoreBackend};
use crate::core::peer_node::NodeEvent;
use crate::core::state::AppState;
use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};

/// 收件箱变化通知事件名（契约第 2 节）。
pub const INBOX_UPDATED_EVENT: &str = "lan-sync://inbox-updated";

/// 设置变化通知事件名（0.2.7）：托盘 / 设置页切换开关后通知前端刷新。
pub const SETTINGS_UPDATED_EVENT: &str = "lan-sync://settings-updated";

/// 共享运行时状态。
pub struct LanSyncShared {
    pub inbox: InboxData,
    pub settings: LanSettings,
    /// 防环：近期接收内容指纹 LRU（契约 5.4）。
    pub received_fingerprints: VecDeque<String>,
    /// 本机 peerId（判断「自己的广播」）。
    pub self_peer_id: String,
    /// 节点是否在运行（状态展示用）。
    pub node_running: bool,
}

static LAN_SYNC: OnceLock<Arc<Mutex<LanSyncShared>>> = OnceLock::new();

/// 获取共享态（setup 后可用）。
pub fn shared() -> Option<&'static Arc<Mutex<LanSyncShared>>> {
    LAN_SYNC.get()
}

/// 初始化（setup 阶段调用一次）：加载设置/收件箱、启动消费者线程、注册广播钩子。
pub fn init(
    app: &AppHandle,
    event_rx: Receiver<NodeEvent>,
    self_peer_id: String,
) -> Result<(), String> {
    let backend = StoreBackend::new(app).map_err(|e| e.message)?;

    // 设置：终端名缺省取主机名并持久化
    let mut settings = backend.load_settings().map_err(|e| e.message)?;
    if settings.terminal_name.trim().is_empty() {
        settings.terminal_name = hostname();
        backend.save_settings(&settings).map_err(|e| e.message)?;
        log::info!(
            "lan_sync: terminal name defaulted to {}",
            settings.terminal_name
        );
    }

    let inbox = backend.load_inbox().map_err(|e| e.message)?;
    log::info!(
        "lan_sync: init peer={self_peer_id} broadcast={} receive={} inbox_nodes={}",
        settings.broadcast_enabled,
        settings.receive_enabled,
        inbox.nodes.len()
    );

    let shared = Arc::new(Mutex::new(LanSyncShared {
        inbox,
        settings,
        received_fingerprints: VecDeque::new(),
        self_peer_id,
        node_running: true,
    }));
    LAN_SYNC
        .set(shared)
        .map_err(|_| "lan-sync already initialized".to_string())?;

    // 消费者线程：节点事件 → 收件箱
    let consumer_app = app.clone();
    std::thread::Builder::new()
        .name("lan-sync-consumer".into())
        .spawn(move || {
            while let Ok(event) = event_rx.recv() {
                if let NodeEvent::PubsubMessage { source, data } = event {
                    handle_message(&consumer_app, &source, &data);
                }
            }
            log::debug!("lan_sync: consumer thread stopped");
        })
        .map_err(|e| format!("spawn consumer thread failed: {e}"))?;

    // 广播钩子：剪贴板历史新条目 → 防环 → 发布（闭包无捕获，参数即回调入参）
    crate::core::hooks::register_new_entry_hook(Box::new(|a, entry| {
        broadcast_captured_entry(a, entry);
    }));

    // 托盘快速开关（core::hooks）：读/写 shared 设置并持久化（与命令同路径）
    crate::core::hooks::register_lan_sync_switches(crate::core::hooks::LanSyncSwitches {
        broadcast_enabled: settings_broadcast_enabled,
        receive_enabled: settings_receive_enabled,
        set_broadcast: set_broadcast_flag,
        set_receive: set_receive_flag,
    });

    Ok(())
}

/// 读取当前广播开关（未初始化返回 false）。
fn settings_broadcast_enabled() -> bool {
    shared()
        .map(|g| g.lock().unwrap().settings.broadcast_enabled)
        .unwrap_or(false)
}

/// 读取当前接收开关（未初始化返回 false）。
fn settings_receive_enabled() -> bool {
    shared()
        .map(|g| g.lock().unwrap().settings.receive_enabled)
        .unwrap_or(false)
}

/// 设置广播开关并持久化（托盘快速开关；与 `set_lan_sync_broadcast` 命令同路径）。
fn set_broadcast_flag(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let Some(shared) = shared() else {
        return Err("lan-sync not initialized".into());
    };
    let settings = {
        let mut g = shared.lock().unwrap();
        g.settings.broadcast_enabled = enabled;
        g.settings.clone()
    };
    StoreBackend::new(app)
        .and_then(|b| b.save_settings(&settings))
        .map_err(|e| e.message)?;
    log::info!("lan_sync: broadcast={enabled} (tray)");
    let _ = app.emit(
        SETTINGS_UPDATED_EVENT,
        serde_json::json!({ "broadcast": enabled }),
    );
    Ok(enabled)
}

/// 设置接收开关并持久化（托盘快速开关；与 `set_lan_sync_receive` 命令同路径）。
fn set_receive_flag(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let Some(shared) = shared() else {
        return Err("lan-sync not initialized".into());
    };
    let settings = {
        let mut g = shared.lock().unwrap();
        g.settings.receive_enabled = enabled;
        g.settings.clone()
    };
    StoreBackend::new(app)
        .and_then(|b| b.save_settings(&settings))
        .map_err(|e| e.message)?;
    log::info!("lan_sync: receive={enabled} (tray)");
    let _ = app.emit(
        SETTINGS_UPDATED_EVENT,
        serde_json::json!({ "receive": enabled }),
    );
    Ok(enabled)
}

/// 本机主机名（设置缺省终端名用）。
fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "VitryTool".into())
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// 通知前端收件箱变化。
fn emit_inbox_updated(app: &AppHandle, reason: &str) {
    let _ = app.emit(INBOX_UPDATED_EVENT, serde_json::json!({ "reason": reason }));
}

/// 持久化收件箱（失败仅日志——内存态仍正确，下次变化会再尝试）。
fn persist_inbox(app: &AppHandle, inbox: &InboxData) {
    if let Err(e) = StoreBackend::new(app).and_then(|b| b.save_inbox(inbox)) {
        log::error!("lan_sync: persist inbox failed: {e}");
    }
}

/// 处理一条收到的 pubsub 消息（消费者线程）。
fn handle_message(app: &AppHandle, source: &str, data: &[u8]) {
    let Ok(env) = serde_json::from_slice::<Envelope>(data) else {
        log::debug!("lan_sync: unparsable message from {source}, ignored");
        return;
    };
    let Some(shared) = shared() else { return };

    // 自身消息 / 接收开关（不入收件箱，也不记防环指纹）
    let (self_peer_id, receive_enabled) = {
        let g = shared.lock().unwrap();
        (g.self_peer_id.clone(), g.settings.receive_enabled)
    };
    if env.peer_id == self_peer_id {
        log::debug!("lan_sync: own message ignored");
        return;
    }
    if !receive_enabled {
        log::debug!("lan_sync: receive disabled, ignored");
        return;
    }

    let Some(entry) = inbox_entry_from_envelope(&env, now_iso()) else {
        log::debug!("lan_sync: empty message from {} ignored", env.peer_id);
        return;
    };
    let fingerprint = entry.fingerprint.clone();

    let (inbox, outcome) = {
        let mut g = shared.lock().unwrap();
        let outcome = insert_message(&mut g.inbox, entry);
        // 防环指纹 LRU：收到（无论新增或去重）都记录
        if !g.received_fingerprints.contains(&fingerprint) {
            g.received_fingerprints.push_back(fingerprint);
            while g.received_fingerprints.len() > MAX_RECEIVED_FINGERPRINTS {
                g.received_fingerprints.pop_front();
            }
        }
        (g.inbox.clone(), outcome)
    };

    persist_inbox(app, &inbox);
    emit_inbox_updated(app, "received");
    match outcome {
        InboxOutcome::New => log::info!("lan_sync: inbox +1 from {}", env.terminal),
        InboxOutcome::DedupPromoted => log::debug!("lan_sync: dedup-promote from {}", env.terminal),
        InboxOutcome::NodeEvicted { evicted_peer_id } => {
            log::info!("lan_sync: evicted node {evicted_peer_id} (bucket limit)")
        }
    }
}

/// 广播钩子：剪贴板历史产生新条目时调用（core::hooks）。
fn broadcast_captured_entry(app: &AppHandle, entry: &serde_json::Value) {
    let Some(shared) = shared() else { return };

    // 开关 / 节点状态 / 身份与终端名
    let (broadcast_enabled, node_running, self_peer_id, terminal_name) = {
        let g = shared.lock().unwrap();
        (
            g.settings.broadcast_enabled,
            g.node_running,
            g.self_peer_id.clone(),
            g.settings.terminal_name.clone(),
        )
    };
    if !broadcast_enabled || !node_running {
        log::debug!("lan_sync: broadcast disabled or node not running, skip");
        return;
    }

    let Some(env) =
        super::service::envelope_from_entry_json(entry, &self_peer_id, &terminal_name, now_ms())
    else {
        log::debug!("lan_sync: entry has no broadcastable content");
        return;
    };
    let Some(fingerprint) = envelope_fingerprint(&env) else {
        return;
    };

    // 防环：近期接收过 → 跳过
    {
        let g = shared.lock().unwrap();
        if g.received_fingerprints.contains(&fingerprint) {
            log::debug!("lan_sync: anti-loop hit, skip broadcast");
            return;
        }
    }

    // 体积上限（契约 5.2：超 1MiB 静默跳过）
    let Ok(bytes) = serde_json::to_vec(&env) else {
        return;
    };
    if bytes.len() > MAX_MESSAGE_BYTES {
        log::info!("lan_sync: message too large ({}B), skipped", bytes.len());
        return;
    }

    // 发布
    let state = app.state::<AppState>();
    let node = state.peer_node.lock().unwrap();
    if let Some(node) = node.as_ref() {
        log::info!("lan_sync: broadcast {}B ({:?})", bytes.len(), env.kinds);
        node.publish(bytes);
    }
}
