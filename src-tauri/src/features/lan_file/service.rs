//! lan-file 业务核心（纯逻辑，无 IO 无 Tauri 依赖，可独立测试）。
//!
//! 契约：`docs/api/lan-file.md` 第 2/3/4/5 节。
//! 包含：响应类型（serde camelCase）、任务状态机、发送侧校验、
//! 公告 peers（TTL 驱逐）、bytesPerSec EMA。
//!
//! 注：类型与常量由 `state.rs`/`commands.rs` 消费；本实现节先行落地并以
//! `#![allow(dead_code)]`（模块级）抑制阶段性未消费告警，接线后移除。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::time::Duration;

// ---------------------------------------------------------------------------
// 常量（契约 5.5/5.6/5.7）
// ---------------------------------------------------------------------------

/// 单任务文件数上限。
pub const MAX_FILES_PER_TASK: usize = 100;
/// 单路径字符上限。
pub const MAX_PATH_LEN: usize = 500;
/// 发送侧重连退避序列（秒，契约 5.5：1/2/4/8/16/32s）。
pub const RETRY_BACKOFF_SECS: [u64; 6] = [1, 2, 4, 8, 16, 32];
/// 断线续传宽限窗口。
pub const RESUME_WINDOW: Duration = Duration::from_secs(120);
/// 提议响应窗口。
pub const OFFER_TIMEOUT: Duration = Duration::from_secs(60);
/// 公告项过期驱逐（12 分钟 = 周期 5min × 2 冗余 + 2min）。
pub const ANNOUNCE_TTL: Duration = Duration::from_secs(12 * 60);
/// 公告周期。
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// 公告主题（契约 5.2）。
pub const ANNOUNCE_TOPIC: &str = "vitrytool-lan-file-announce";
/// 公告协议版本。
pub const ANNOUNCE_VERSION: &str = "0.3.0";
/// 图片通道队列深度（超出丢最旧，契约 5.3）。
pub const IMAGE_QUEUE_CAP: usize = 8;
/// 进度节流：≤4 次/秒。
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// **单块写入停滞判定**（不是帧级硬超时）：一个 1MiB 块连续 30s 写不进内核缓冲，
/// 说明对端/链路已零进展（真机实测：WiFi 丢包使 cwnd 塌到 2 段、RTT 升到秒级，
/// 传输看着像「卡死几分钟」）。触发后按断线处理 → 重连 + 断点续传，
/// 比在塌缩的连接上干等快得多。
pub const CHUNK_STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// 接收侧单块等待超过该值仅**记日志**（定位卡顿用，不中断传输）。
pub const RECV_STALL_LOG_THRESHOLD: Duration = Duration::from_secs(5);
/// 接收侧**彻底收不到数据**的判定（远超发送侧 30s 停滞阈值）：按断线处理 →
/// 释放会话槽 + 保留 sidecar 进续传窗口，让对端的重连能立刻被接受。
pub const RECV_DEAD_TIMEOUT: Duration = Duration::from_secs(60);
/// bytesPerSec EMA α。
pub const EMA_ALPHA: f64 = 0.3;

// ---------------------------------------------------------------------------
// 响应类型（契约第 3 节；serde camelCase 与 TS 一一对应）
// ---------------------------------------------------------------------------

/// `getLanFileStatus` 响应。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanFileStatus {
    pub enabled: bool,
    pub listening: bool,
    pub tcp_port: Option<u16>,
    pub peer_count: usize,
    pub trusted_count: usize,
}

/// `getLanFilePeers` 元素。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanFilePeer {
    pub peer_id: String,
    pub terminal_name: String,
    pub fingerprint: String,
    pub trusted: bool,
    pub supports_interactive: bool,
}

/// `getLanFilePeers` 响应。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanFilePeersResp {
    pub peers: Vec<LanFilePeer>,
}

/// `lan-file://incoming` 载荷。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanFileOffer {
    pub transfer_id: String,
    pub peer_id: String,
    pub terminal_name: String,
    pub fingerprint: String,
    pub name_clash: bool,
    pub files: Vec<OfferFileInfo>,
    pub total_bytes: u64,
    /// 该 peerId 上次出现在公告列表距今（分钟；TOFU 参考信息，无记录为 0）。
    pub known_from_minutes: u64,
}

/// 提议中的单文件（展示名已净化）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OfferFileInfo {
    pub name: String,
    pub size: u64,
}

/// 文件传输子状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Pending,
    Transferring,
    Done,
}

/// `lan-file://transfer-updated` 载荷（活跃任务全量快照）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanFileTransfer {
    /// null = 空闲（回到空闲时发一次）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer_id: Option<String>,
    pub direction: String, // "send" | "receive"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_name: Option<String>,
    pub state: TransferState,
    pub files: Vec<TransferFileInfo>,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    /// 后端 EMA（指数滑动平均，窗口 ~3s；resuming 期不刷新）。非传输态为 0。
    pub bytes_per_sec: f64,
    /// 仅接收侧 done：落盘最终路径（「打开所在位置」用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_paths: Option<Vec<String>>,
    /// 稳定错误码，前端 i18n 翻译。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TransferError>,
}

/// 文件进度行。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TransferFileInfo {
    pub name: String,
    pub size: u64,
    pub transferred_bytes: u64,
    pub status: FileStatus,
}

/// 任务错误（稳定码 + 插值参数）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TransferError {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

/// 任务状态（契约 5.3 状态机）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferState {
    Offering,
    Transferring,
    Resuming,
    Done,
    Failed,
    Cancelled,
    Rejected,
}

/// `sendLanFile` 响应。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SendLanFileResp {
    pub transfer_id: String,
}

/// 错误码（契约第 4 节）。
pub mod err {
    pub const BUSY: &str = "lan_file.busy";
    pub const NOT_ENABLED: &str = "lan_file.not_enabled";
    pub const PEER_UNSUPPORTED: &str = "lan_file.peer_unsupported";
    pub const PEER_NOT_FOUND: &str = "lan_file.peer_not_found";
    pub const FILE_NOT_FOUND: &str = "lan_file.file_not_found";
    pub const FILE_UNREADABLE: &str = "lan_file.file_unreadable";
    pub const INVALID_PATH: &str = "lan_file.invalid_path";
    pub const DISK_FULL: &str = "lan_file.disk_full";
    pub const OFFER_TIMEOUT: &str = "lan_file.offer_timeout";
    pub const REJECTED: &str = "lan_file.rejected";
    pub const INTEGRITY_MISMATCH: &str = "lan_file.integrity_mismatch";
    pub const TRANSFER_FAILED: &str = "lan_file.transfer_failed";
    pub const CANCELLED: &str = "lan_file.cancelled";
    pub const STORAGE_ERROR: &str = "lan_file.storage_error";
    pub const NODE_NOT_RUNNING: &str = "lan_file.node_not_running";
}

// ---------------------------------------------------------------------------
// 发送侧输入校验（契约 5.6，注入 fs 探针保持纯函数）
// ---------------------------------------------------------------------------

/// 发送侧路径校验（契约 5.6）：数量 ≤100、路径 ≤500 字符、存在且为普通文件、可打开。
///
/// `probe`：注入的文件探针（Ok(Some(size)) = 普通文件；Ok(None) =不存在；
/// Err(()) = 存在但非普通文件或不可读）。返回 Ok(总字节) 或 Err(稳定错误码)。
pub fn validate_send_paths(
    paths: &[String],
    probe: &dyn Fn(&str) -> Result<Option<u64>, ()>,
) -> Result<u64, String> {
    if paths.is_empty() || paths.len() > MAX_FILES_PER_TASK {
        return Err(err::INVALID_PATH.to_string());
    }
    let mut total = 0u64;
    for p in paths {
        if p.is_empty() || p.chars().count() > MAX_PATH_LEN {
            return Err(err::INVALID_PATH.to_string());
        }
        match probe(p) {
            Ok(Some(size)) => total = total.saturating_add(size),
            Ok(None) => return Err(err::FILE_NOT_FOUND.to_string()),
            Err(()) => return Err(err::FILE_UNREADABLE.to_string()),
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// 路径预检（契约 5.6 增量：`checkLanFilePaths`）
// ---------------------------------------------------------------------------

/// 路径类型（预检结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// 普通文件（字节数）。
    File(u64),
    /// 目录（v1 不支持传输，前端据此忽略并提示）。
    Dir,
    /// 不存在。
    Missing,
    /// 存在但不可读 / 非普通文件（设备文件、权限不足等）。
    Unreadable,
}

/// 单个路径的类型判定（真实 fs）。
pub fn path_kind(path: &str) -> PathKind {
    let p = std::path::Path::new(path);
    match std::fs::metadata(p) {
        Ok(m) if m.is_dir() => PathKind::Dir,
        Ok(m) if m.is_file() => match std::fs::File::open(p) {
            Ok(_) => PathKind::File(m.len()),
            Err(_) => PathKind::Unreadable,
        },
        Ok(_) => PathKind::Unreadable,
        Err(_) => PathKind::Missing,
    }
}

/// 路径预检条目（`checkLanFilePaths` 响应元素）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanFilePathInfo {
    pub path: String,
    /// 净化后的展示名（与落盘/传输展示一致）。
    pub name: String,
    /// 字节数（非普通文件为 0）。
    pub size: u64,
    /// 是否为目录（v1 不支持：前端忽略并提示）。
    pub is_dir: bool,
    /// 是否可直接发送（普通文件且可读）。
    pub readable: bool,
}

/// `checkLanFilePaths` 响应。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanFilePathsResp {
    pub paths: Vec<LanFilePathInfo>,
}

/// 预检单个路径（不建任务，只回答「能不能传」）。
pub fn path_info(path: &str) -> LanFilePathInfo {
    let kind = path_kind(path);
    let name = path
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .map(super::store::sanitize_filename)
        .unwrap_or_else(|| "unnamed".to_string());
    LanFilePathInfo {
        path: path.to_string(),
        name,
        size: match kind {
            PathKind::File(n) => n,
            _ => 0,
        },
        is_dir: kind == PathKind::Dir,
        readable: matches!(kind, PathKind::File(_)),
    }
}

/// 发送侧文件探测实现（真实 fs；命令层用）。
pub fn probe_regular_file(path: &str) -> Result<Option<u64>, ()> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Ok(None);
    }
    match std::fs::metadata(p) {
        Ok(m) if m.is_file() => match std::fs::File::open(p) {
            Ok(_) => Ok(Some(m.len())),
            Err(_) => Err(()),
        },
        Ok(_) => Err(()), // 目录等非普通文件
        Err(_) => Err(()),
    }
}

// ---------------------------------------------------------------------------
// 公告 peers（TTL 驱逐，契约 5.2）
// ---------------------------------------------------------------------------

/// 公告负载（gossipsub 主题 `vitrytool-lan-file-announce`，契约 5.2）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Announce {
    pub v: String,
    pub peer_id: String,
    pub terminal: String,
    pub tcp_port: u16,
    pub fingerprint: String,
    /// `file` = 支持人工确认传输（桌面），`img` = 支持图片通道接收。
    pub caps: Vec<String>,
    /// 公告生成时刻（unix 毫秒；仅用于**同一 peerId** 的新旧判定，跨机时钟差不影响）。
    ///
    /// 用途：进程重启 / 快速开关时，旧实例的撤销公告可能晚于新实例的公告到达；
    /// 不做新旧判定会把已上线的对端误删（真机实测）。旧版公告缺此字段 → 默认 0（最旧）。
    #[serde(default)]
    pub ts: u64,
}

impl Announce {
    /// 是否支持人工确认传输（caps 含 `file`）。
    pub fn supports_interactive(&self) -> bool {
        self.caps.iter().any(|c| c == "file")
    }

    /// 是否支持图片通道接收（caps 含 `img`）。
    pub fn supports_image(&self) -> bool {
        self.caps.iter().any(|c| c == "img")
    }
}

/// 一条在线公告（含最近收到时间，驱逐用）。
#[derive(Debug, Clone, PartialEq)]
pub struct AnnounceEntry {
    pub announce: Announce,
    /// 最近一次收到公告（单调毫秒）。
    pub last_seen_ms: u64,
}

/// 公告是否**过期**（比已记录的同一 peerId 公告更旧）——乱序到达的旧公告/撤销公告
/// 一律忽略（真机实测：快速重启时旧实例的撤销公告会误删已上线对端）。
///
/// 只比较同一 peerId 的 ts，故机器间时钟差不影响判定。
pub fn announce_is_stale(peers: &[AnnounceEntry], announce: &Announce) -> bool {
    peers
        .iter()
        .find(|e| e.announce.peer_id == announce.peer_id)
        .map(|e| announce.ts < e.announce.ts)
        .unwrap_or(false)
}

/// 合并一条公告（upsert + 刷新 lastSeen；同 peerId 端口变化以新公告为准）。
pub fn upsert_announce(peers: &mut Vec<AnnounceEntry>, announce: Announce, now_ms: u64) {
    if let Some(existing) = peers
        .iter_mut()
        .find(|e| e.announce.peer_id == announce.peer_id)
    {
        existing.announce = announce;
        existing.last_seen_ms = now_ms;
        return;
    }
    peers.push(AnnounceEntry {
        announce,
        last_seen_ms: now_ms,
    });
}

/// 过期驱逐（契约 5.2：12 分钟未见重公告 → 移除）。返回是否发生移除。
pub fn evict_expired(peers: &mut Vec<AnnounceEntry>, now_ms: u64) -> bool {
    let before = peers.len();
    let ttl_ms = ANNOUNCE_TTL.as_millis() as u64;
    peers.retain(|e| now_ms.saturating_sub(e.last_seen_ms) < ttl_ms);
    peers.len() != before
}

/// 查询 peerId 的公告。
pub fn find_announce<'a>(peers: &'a [AnnounceEntry], peer_id: &str) -> Option<&'a Announce> {
    peers
        .iter()
        .find(|e| e.announce.peer_id == peer_id)
        .map(|e| &e.announce)
}

/// 公告是否为撤销（`caps` 为空 = 关闭开关 / 退出时的尽力撤销，契约 5.2）。
pub fn is_revoke_announce(announce: &Announce) -> bool {
    announce.caps.is_empty()
}

/// 从 peers 移除该终端（收到撤销公告；返回是否有变化）。
pub fn remove_announce(peers: &mut Vec<AnnounceEntry>, peer_id: &str) -> bool {
    let before = peers.len();
    peers.retain(|e| e.announce.peer_id != peer_id);
    peers.len() != before
}

/// 是否需要在收到某公告后**补发自身公告**：仅当该对端首次出现在列表（契约 5.2）。
///
/// 反例（真机实测的严重 bug）：对每条收到的公告都补发自身公告 → 两端互相触发，
/// gossipsub 消息 id 含 seqno 导致去重失效 → 公告回声风暴（每端数十条/秒、日志刷屏）。
pub fn announce_back_needed(peers: &[AnnounceEntry], peer_id: &str) -> bool {
    !peers.iter().any(|e| e.announce.peer_id == peer_id)
}

/// 续传偏移对应的分块序号（发送方 seek 后从该块继续、接收方按同一序号校验 AAD）。
///
/// 真机实测教训：发送方曾从 0 重新计数、接收方按 `offset/CHUNK_SIZE` 计数，
/// 两侧 seq 不一致 → 每个 Chunk 的 AAD 校验失败 → 续传永远失败。
pub fn chunk_seq_at(offset: u64) -> u32 {
    (offset / super::transport::proto::CHUNK_SIZE as u64) as u32
}

/// 公告自校验（契约 5.2：multihash(公钥)==peerId 不一致即丢弃）。
pub fn announce_consistent(announce: &Announce) -> bool {
    // fingerprint = "SHA256:" + base64(ed25519 公钥)；peerId = multihash(公钥 protobuf)。
    // 校验：base64 解码公钥 → peerId 重推导（crypto::peer_id_from_ed25519 同构）→ 比对。
    use base64::Engine as _;
    let Some(pub_b64) = announce.fingerprint.strip_prefix("SHA256:") else {
        return false;
    };
    let Ok(pubkey) = base64::engine::general_purpose::STANDARD.decode(pub_b64) else {
        return false;
    };
    let Ok(bytes): Result<[u8; 32], _> = pubkey.clone().try_into() else {
        return false;
    };
    let Ok(pubkey_obj) = ed25519_dalek::VerifyingKey::from_bytes(&bytes) else {
        return false;
    };
    super::transport::crypto::peer_id_from_ed25519(&pubkey_obj) == announce.peer_id
}

// ---------------------------------------------------------------------------
// 进度速率 EMA（契约 5.3：α≈0.3/次更新，窗口 ~3s）
// ---------------------------------------------------------------------------

/// EMA 速率估计器（bytes/sec）。
#[derive(Debug, Clone, PartialEq)]
pub struct RateEstimator {
    ema_bps: f64,
    last_bytes: u64,
    last_ms: u64,
}

impl RateEstimator {
    pub fn new() -> Self {
        Self {
            ema_bps: 0.0,
            last_bytes: 0,
            last_ms: 0,
        }
    }

    /// 重置（新文件 / 恢复传输时）。
    pub fn reset(&mut self, bytes_now: u64, now_ms: u64) {
        self.last_bytes = bytes_now;
        self.last_ms = now_ms;
        // ema 保留既有平滑值（断线重连不清零，resuming 期不更新）
    }

    /// 观测一次进度；返回刷新后的 EMA（bytes/sec）。
    pub fn observe(&mut self, bytes_now: u64, now_ms: u64) -> f64 {
        if now_ms <= self.last_ms {
            return self.ema_bps;
        }
        let dt = (now_ms - self.last_ms) as f64 / 1000.0;
        let inst = (bytes_now.saturating_sub(self.last_bytes)) as f64 / dt;
        if self.ema_bps == 0.0 {
            self.ema_bps = inst;
        } else {
            self.ema_bps = EMA_ALPHA * inst + (1.0 - EMA_ALPHA) * self.ema_bps;
        }
        self.last_bytes = bytes_now;
        self.last_ms = now_ms;
        self.ema_bps
    }

    pub fn value(&self) -> f64 {
        self.ema_bps
    }
}

impl Default for RateEstimator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 续传窗口判定（契约 5.5，注入时钟）
// ---------------------------------------------------------------------------

/// 判断 sidecar 是否仍在续传窗口内（上次活动 now - last_active < 120s）。
pub fn resume_window_active(last_active_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_active_ms) < RESUME_WINDOW.as_millis() as u64
}

/// 发起方下次重连等待（指数退避 1/2/4/8/16/32s，封顶 32s；attempt 从 0 起）。
pub fn retry_delay(attempt: u32) -> Duration {
    let idx = (attempt as usize).min(RETRY_BACKOFF_SECS.len() - 1);
    Duration::from_secs(RETRY_BACKOFF_SECS[idx])
}

// ---------------------------------------------------------------------------
// 终态映射与错误码归一（契约 5.3 状态机 / 第 4 节错误码）
// ---------------------------------------------------------------------------

/// 任务错误串 → 稳定错误码：取 `:` 前的首段，非 `lan_file.*` 前缀一律归 `transfer_failed`。
///
/// 任务内部用 `format!("{}: {io}", err::TRANSFER_FAILED)` 携带诊断细节，
/// 前端只认稳定码，故此处剥离细节；对端 `Reject{code}` 的码原样保留。
pub fn stable_error_code(raw: &str) -> String {
    let head = raw.split(':').next().unwrap_or("").trim();
    if head == PEER_CANCEL_MARK {
        return err::CANCELLED.to_string();
    }
    if head.starts_with("lan_file.") && KNOWN_ERROR_CODES.contains(&head) {
        head.to_string()
    } else {
        err::TRANSFER_FAILED.to_string()
    }
}

/// 内部标记：**对端**显式取消（映射为契约码 `lan_file.cancelled`）。
///
/// 本地取消（用户点「取消传输」/ 关开关）用裸 `lan_file.cancelled` 且**不带错误文案**
/// （状态 `cancelled` 已表达，避免把「自己取消」显示成「对方取消了传输」）。
pub const PEER_CANCEL_MARK: &str = "lan_file.peer_cancelled";

/// 全部稳定错误码（契约第 4 节，15 个）。
pub const KNOWN_ERROR_CODES: [&str; 15] = [
    err::BUSY,
    err::NOT_ENABLED,
    err::PEER_UNSUPPORTED,
    err::PEER_NOT_FOUND,
    err::FILE_NOT_FOUND,
    err::FILE_UNREADABLE,
    err::INVALID_PATH,
    err::DISK_FULL,
    err::OFFER_TIMEOUT,
    err::REJECTED,
    err::INTEGRITY_MISMATCH,
    err::TRANSFER_FAILED,
    err::CANCELLED,
    err::STORAGE_ERROR,
    err::NODE_NOT_RUNNING,
];

/// 发送侧终态映射（契约 5.3）：取消 → cancelled，对端拒绝 → rejected，其余失败 → failed。
pub fn send_terminal_state(code: &str) -> TransferState {
    match code {
        err::CANCELLED => TransferState::Cancelled,
        err::REJECTED => TransferState::Rejected,
        _ => TransferState::Failed,
    }
}

// ---------------------------------------------------------------------------
// 续传窗口残留清理（契约 5.5：启动时扫 `.tmp`/sidecar，超窗直接清理）
// ---------------------------------------------------------------------------

/// 残留传输工件（启动清理的输入项）。
#[derive(Debug, Clone, PartialEq)]
pub struct TransferArtifact {
    /// 工件文件名（sidecar 为 `.<id>.meta.json`，临时文件为 `.<id>.<i>.tmp`）。
    pub name: String,
    /// 最后修改时间（unix 毫秒）。
    pub mtime_ms: u64,
}

/// 挑选需要清理的残留工件：mtime 早于 `now - window` 的一律清理（契约 5.5）。
///
/// 只认本功能的命名（`.<id>.meta.json` / `.<id>.<i>.tmp`），其余文件不动。
pub fn stale_transfer_artifacts(
    artifacts: &[TransferArtifact],
    now_ms: u64,
    window: Duration,
) -> Vec<String> {
    let window_ms = window.as_millis() as u64;
    artifacts
        .iter()
        .filter(|a| is_transfer_artifact(&a.name))
        .filter(|a| now_ms.saturating_sub(a.mtime_ms) > window_ms)
        .map(|a| a.name.clone())
        .collect()
}

/// 文件名是否本功能的传输工件（sidecar / `.tmp`）。
pub fn is_transfer_artifact(name: &str) -> bool {
    if !name.starts_with('.') {
        return false;
    }
    name.ends_with(".meta.json") || name.ends_with(".tmp")
}

// ---------------------------------------------------------------------------
// 图片通道来源门控（契约 5.7-2 / 5.9）
// ---------------------------------------------------------------------------

/// 剪贴板主题新鲜观察窗口（移动端免人工信任的第二层证据，契约 5.9）。
pub const CLIPBOARD_FRESH_WINDOW: Duration = Duration::from_secs(5 * 60);

/// peerId 是否在新鲜窗口内于剪贴板主题上出现过（移动端免确认落图依据）。
pub fn clipboard_peer_fresh(last_seen_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_seen_ms) < CLIPBOARD_FRESH_WINDOW.as_millis() as u64
}

/// 图片通道来源是否可信（契约 5.7-2 桌面 / 5.9 移动端）。
///
/// - 桌面：来源 peerId ∈ TOFU 信任表；
/// - 移动端：无信任 UI 载体 → 信任表恒空，改由「noise 认证 + 5 分钟剪贴板新鲜观察」
///   两层证据放行（noise 认证由 VLF 握手校验 `multihash(公钥)==peerId` 保证）。
pub fn image_source_admissible(trusted: bool, clipboard_fresh: bool, mobile: bool) -> bool {
    trusted || (mobile && clipboard_fresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_info_classifies_file_dir_and_missing() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("vitry-pathinfo-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        fs::write(&file, b"hello").unwrap();

        let f = path_info(file.to_str().unwrap());
        assert_eq!(f.name, "a.txt");
        assert_eq!(f.size, 5);
        assert!(!f.is_dir);
        assert!(f.readable);

        // 目录：v1 不支持 → is_dir=true、readable=false、size=0
        let d = path_info(dir.to_str().unwrap());
        assert!(d.is_dir);
        assert!(!d.readable);
        assert_eq!(d.size, 0);

        // 不存在
        let missing = path_info(dir.join("nope.txt").to_str().unwrap());
        assert!(!missing.is_dir);
        assert!(!missing.readable);
        assert_eq!(missing.size, 0);

        // 空路径 / 目录穿越名 → 展示名净化
        assert_eq!(path_info("").name, "unnamed");
        assert_eq!(path_info("/etc/../x/b.txt").name, "b.txt");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_paths_rules() {
        let ok = |_: &str| Ok::<Option<u64>, ()>(Some(10));
        // 空列表 / 超量
        assert_eq!(validate_send_paths(&[], &ok), Err(err::INVALID_PATH.into()));
        let many: Vec<String> = (0..101).map(|i| format!("f{i}")).collect();
        assert_eq!(
            validate_send_paths(&many, &ok),
            Err(err::INVALID_PATH.into())
        );
        // 超长路径
        let long = "x".repeat(501);
        assert_eq!(
            validate_send_paths(&[long], &ok),
            Err(err::INVALID_PATH.into())
        );
        // 不存在
        let none = |_: &str| Ok::<Option<u64>, ()>(None);
        assert_eq!(
            validate_send_paths(&["missing".into()], &none),
            Err(err::FILE_NOT_FOUND.into())
        );
        // 目录 / 不可读
        let dir = |_: &str| Err::<Option<u64>, ()>(());
        assert_eq!(
            validate_send_paths(&["dir".into()], &dir),
            Err(err::FILE_UNREADABLE.into())
        );
        // 正常：求和
        let sizes = |p: &str| Ok::<Option<u64>, ()>(Some(p.len() as u64));
        assert_eq!(
            validate_send_paths(&["aa".into(), "bbb".into()], &sizes),
            Ok(5)
        );
        // 恰好 100 个合法
        let hundred: Vec<String> = (0..100).map(|i| format!("f{i}")).collect();
        assert!(validate_send_paths(&hundred, &ok).is_ok());
    }

    #[test]
    fn announce_upsert_evict_ttl() {
        let a = Announce {
            v: ANNOUNCE_VERSION.into(),
            peer_id: "peerA".into(),
            terminal: "A".into(),
            tcp_port: 1000,
            fingerprint: "SHA256:x".into(),
            caps: vec!["file".into(), "img".into()],
            ts: 0,
        };
        let mut peers = Vec::new();
        upsert_announce(&mut peers, a.clone(), 0);
        // 更新端口刷新时间
        let mut a2 = a.clone();
        a2.tcp_port = 2000;
        upsert_announce(&mut peers, a2, 5000);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].announce.tcp_port, 2000);
        assert_eq!(peers[0].last_seen_ms, 5000);
        // 未过期
        assert!(!evict_expired(&mut peers, 5000 + 11 * 60 * 1000));
        // 过期（12min）
        assert!(evict_expired(&mut peers, 5000 + 12 * 60 * 1000 + 1));
        assert!(peers.is_empty());
    }

    #[test]
    fn announce_caps_semantics() {
        let a = Announce {
            v: ANNOUNCE_VERSION.into(),
            peer_id: "p".into(),
            terminal: "t".into(),
            tcp_port: 1,
            fingerprint: "SHA256:x".into(),
            caps: vec!["img".into()],
            ts: 0,
        };
        assert!(a.supports_image());
        assert!(!a.supports_interactive(), "移动端仅 img");
        let mut b = a.clone();
        b.caps = vec!["file".into()];
        assert!(b.supports_interactive());
        assert!(!b.supports_image());
    }

    #[test]
    fn resume_chunk_seq_is_shared_by_both_sides() {
        use crate::features::lan_file::transport::proto::CHUNK_SIZE;
        assert_eq!(chunk_seq_at(0), 0);
        assert_eq!(chunk_seq_at(CHUNK_SIZE as u64), 1);
        assert_eq!(chunk_seq_at(CHUNK_SIZE as u64 * 9), 9);
        assert_eq!(chunk_seq_at(CHUNK_SIZE as u64 + 5), 1, "非整块偏移归所在块");
    }

    #[test]
    fn stale_announce_is_ignored() {
        let mk = |peer: &str, ts: u64, caps: Vec<String>| Announce {
            v: ANNOUNCE_VERSION.into(),
            peer_id: peer.into(),
            terminal: "T".into(),
            tcp_port: 1,
            fingerprint: "SHA256:x".into(),
            caps,
            ts,
        };
        let mut peers = Vec::new();
        upsert_announce(&mut peers, mk("peerA", 1000, vec!["file".into()]), 0);
        // 更新的公告 → 不 stale
        assert!(!announce_is_stale(
            &peers,
            &mk("peerA", 2000, vec!["file".into()])
        ));
        // 旧公告 / 旧实例的撤销公告 → stale（真机实测：快速重启会误删已上线对端）
        assert!(announce_is_stale(&peers, &mk("peerA", 999, vec![])));
        assert!(announce_is_stale(
            &peers,
            &mk("peerA", 1, vec!["file".into()])
        ));
        // 同一 ts 幂等重发 → 不 stale
        assert!(!announce_is_stale(
            &peers,
            &mk("peerA", 1000, vec!["file".into()])
        ));
        // 未知对端 → 不 stale
        assert!(!announce_is_stale(&peers, &mk("peerB", 1, vec![])));
        // 缺 ts 字段的旧版公告（serde default 0）解析为最旧
        let old: Announce =
            serde_json::from_str(r#"{"v":"0.3.0","peerId":"p","terminal":"T","tcpPort":1,"fingerprint":"SHA256:x","caps":["file"]}"#)
                .unwrap();
        assert_eq!(old.ts, 0);
    }

    #[test]
    fn announce_revoke_removes_peer() {
        let mk = |caps: Vec<String>| Announce {
            v: ANNOUNCE_VERSION.into(),
            peer_id: "peerA".into(),
            terminal: "T".into(),
            tcp_port: 100,
            fingerprint: "SHA256:x".into(),
            caps,
            ts: 0,
        };
        // 空 caps = 撤销公告
        assert!(is_revoke_announce(&mk(vec![])));
        assert!(!is_revoke_announce(&mk(vec!["file".into()])));
        let mut peers = Vec::new();
        upsert_announce(&mut peers, mk(vec!["file".into(), "img".into()]), 1000);
        assert_eq!(peers.len(), 1);
        // 撤销 → 移除
        assert!(remove_announce(&mut peers, "peerA"));
        assert!(peers.is_empty());
        // 重复撤销无变化
        assert!(!remove_announce(&mut peers, "peerA"));
    }

    #[test]
    fn peer_cancel_mark_maps_to_contract_code() {
        // 对端取消用内部标记，映射到契约码 lan_file.cancelled；
        // 本地取消用裸 lan_file.cancelled（前端据此不显示「对方取消了传输」）。
        assert_eq!(stable_error_code(PEER_CANCEL_MARK), err::CANCELLED);
        assert_eq!(
            stable_error_code("lan_file.peer_cancelled: user"),
            err::CANCELLED
        );
        assert_eq!(stable_error_code(err::CANCELLED), err::CANCELLED);
    }

    #[test]
    fn announce_back_only_on_first_sight() {
        // 真机 bug 回归：收到对端公告后补发自身公告必须**仅限首次**，
        // 否则两端互相触发形成公告回声风暴（gossipsub seqno 使去重失效）。
        let mk = |id: &str| Announce {
            v: ANNOUNCE_VERSION.into(),
            peer_id: id.into(),
            terminal: "T".into(),
            tcp_port: 1,
            fingerprint: "SHA256:x".into(),
            caps: vec!["file".into()],
            ts: 0,
        };
        let mut peers = Vec::new();
        assert!(announce_back_needed(&peers, "peerA"), "首次见到 → 补发");
        upsert_announce(&mut peers, mk("peerA"), 1000);
        assert!(!announce_back_needed(&peers, "peerA"), "已知对端 → 不补发");
        assert!(announce_back_needed(&peers, "peerB"), "另一个新对端 → 补发");
        // 重公告（同 peerId 再次到达）同样不补发
        upsert_announce(&mut peers, mk("peerA"), 2000);
        assert!(!announce_back_needed(&peers, "peerA"));
    }

    #[test]
    fn rate_ema_window_and_freeze_on_resume() {
        let mut r = RateEstimator::new();
        r.reset(0, 0);
        // 1s 内传 3000B → inst 3000B/s，EMA=3000
        assert_eq!(r.observe(3000, 1000), 3000.0);
        // 又 1s 传 3000B → inst 3000 → EMA 3000
        assert_eq!(r.observe(6000, 2000), 3000.0);
        // 突发：1s 传 9000B → inst 9000 → EMA = 0.3*9000 + 0.7*3000 = 4800
        assert_eq!(r.observe(15000, 3000), 4800.0);
        // 同时刻重复观测不刷新
        assert_eq!(r.observe(15000, 3000), 4800.0);
        // resuming 期（约定不调用 observe，reset 不清零 EMA）
        r.reset(15000, 5000);
        assert_eq!(r.value(), 4800.0);
        assert_eq!(EMA_ALPHA, 0.3);
    }

    #[test]
    fn resume_window_and_backoff() {
        assert!(resume_window_active(0, 119_999));
        assert!(!resume_window_active(0, 120_000));
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(1), Duration::from_secs(2));
        assert_eq!(retry_delay(2), Duration::from_secs(4));
        assert_eq!(retry_delay(5), Duration::from_secs(32));
        assert_eq!(retry_delay(99), Duration::from_secs(32), "封顶 32s");
    }

    #[test]
    fn transfer_snapshot_serialization_shape() {
        let t = LanFileTransfer {
            transfer_id: Some("t1".into()),
            direction: "send".into(),
            peer_id: Some("p".into()),
            terminal_name: Some("T".into()),
            state: TransferState::Transferring,
            files: vec![TransferFileInfo {
                name: "a".into(),
                size: 10,
                transferred_bytes: 5,
                status: FileStatus::Transferring,
            }],
            total_bytes: 10,
            transferred_bytes: 5,
            bytes_per_sec: 1024.0,
            saved_paths: None,
            error: None,
        };
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["transferId"], "t1");
        assert_eq!(json["state"], "transferring");
        assert_eq!(json["files"][0]["status"], "transferring");
        // 空闲快照：transferId 缺省（null 语义由前端处理）
        let idle = LanFileTransfer {
            transfer_id: None,
            direction: "send".into(),
            peer_id: None,
            terminal_name: None,
            state: TransferState::Done,
            files: vec![],
            total_bytes: 0,
            transferred_bytes: 0,
            bytes_per_sec: 0.0,
            saved_paths: None,
            error: None,
        };
        let json = serde_json::to_value(&idle).unwrap();
        assert!(json.get("transferId").is_none());
    }

    #[test]
    fn constants_match_contract() {
        assert_eq!(MAX_FILES_PER_TASK, 100);
        assert_eq!(MAX_PATH_LEN, 500);
        assert_eq!(RETRY_BACKOFF_SECS, [1, 2, 4, 8, 16, 32]);
        assert_eq!(RESUME_WINDOW, Duration::from_secs(120));
        assert_eq!(OFFER_TIMEOUT, Duration::from_secs(60));
        assert_eq!(ANNOUNCE_TTL, Duration::from_secs(720));
        assert_eq!(ANNOUNCE_INTERVAL, Duration::from_secs(300));
        assert_eq!(ANNOUNCE_TOPIC, "vitrytool-lan-file-announce");
        assert_eq!(IMAGE_QUEUE_CAP, 8);
        assert_eq!(PROGRESS_INTERVAL, Duration::from_millis(250));
        assert_eq!(CLIPBOARD_FRESH_WINDOW, Duration::from_secs(300));
        assert_eq!(KNOWN_ERROR_CODES.len(), 15, "契约第 4 节 15 个错误码");
        // 停滞检测：发送侧先动（30s），接收侧看门狗更晚（60s）才放行会话槽
        assert_eq!(CHUNK_STALL_TIMEOUT, Duration::from_secs(30));
        assert_eq!(RECV_DEAD_TIMEOUT, Duration::from_secs(60));
        assert!(CHUNK_STALL_TIMEOUT < RECV_DEAD_TIMEOUT);
        assert!(RECV_DEAD_TIMEOUT < RESUME_WINDOW, "看门狗必须在续传窗口内");
    }

    // ---- 终态映射与错误码归一（契约 5.3 / 第 4 节） ----

    #[test]
    fn stable_error_code_strips_detail_and_normalizes() {
        assert_eq!(stable_error_code(err::CANCELLED), err::CANCELLED);
        assert_eq!(stable_error_code(err::REJECTED), err::REJECTED);
        // 任务内部携带诊断细节
        assert_eq!(
            stable_error_code("lan_file.transfer_failed: connection reset by peer"),
            err::TRANSFER_FAILED
        );
        // 对端 Reject 的码原样保留
        assert_eq!(stable_error_code(err::DISK_FULL), err::DISK_FULL);
        // 未知前缀 / 空串 / 非 lan_file 域 → transfer_failed
        assert_eq!(stable_error_code("boom"), err::TRANSFER_FAILED);
        assert_eq!(stable_error_code(""), err::TRANSFER_FAILED);
        assert_eq!(
            stable_error_code("lan_file.nonexistent"),
            err::TRANSFER_FAILED
        );
    }

    #[test]
    fn send_terminal_state_mapping() {
        assert_eq!(
            send_terminal_state(err::CANCELLED),
            TransferState::Cancelled
        );
        assert_eq!(send_terminal_state(err::REJECTED), TransferState::Rejected);
        assert_eq!(
            send_terminal_state(err::OFFER_TIMEOUT),
            TransferState::Failed
        );
        assert_eq!(
            send_terminal_state(err::TRANSFER_FAILED),
            TransferState::Failed
        );
        assert_eq!(send_terminal_state(err::DISK_FULL), TransferState::Failed);
    }

    // ---- 启动残留清理（契约 5.5） ----

    #[test]
    fn stale_artifacts_only_own_names_and_expired() {
        let window = RESUME_WINDOW;
        let now = 1_000_000u64;
        let arts = vec![
            // 超窗 sidecar（清）
            TransferArtifact {
                name: ".tid-1.meta.json".into(),
                mtime_ms: now - 120_001,
            },
            // 窗口内 sidecar（留，等对端重连）
            TransferArtifact {
                name: ".tid-2.meta.json".into(),
                mtime_ms: now - 119_000,
            },
            // 超窗 tmp（清）
            TransferArtifact {
                name: ".tid-1.0.tmp".into(),
                mtime_ms: now - 300_000,
            },
            // 非本功能命名（一律不动）
            TransferArtifact {
                name: "普通文件.txt".into(),
                mtime_ms: now - 300_000,
            },
            TransferArtifact {
                name: ".hidden".into(),
                mtime_ms: now - 300_000,
            },
            TransferArtifact {
                name: "notes.meta.json".into(),
                mtime_ms: now - 300_000,
            },
        ];
        let stale = stale_transfer_artifacts(&arts, now, window);
        assert_eq!(stale, vec![".tid-1.meta.json", ".tid-1.0.tmp"]);
        assert!(is_transfer_artifact(".abc.meta.json"));
        assert!(is_transfer_artifact(".abc.3.tmp"));
        assert!(!is_transfer_artifact("abc.meta.json"));
        assert!(!is_transfer_artifact(".abc.txt"));
    }

    // ---- 图片通道来源门控（契约 5.7-2 / 5.9） ----

    #[test]
    fn image_source_gate_matrix() {
        // 桌面：只认信任表
        assert!(image_source_admissible(true, false, false));
        assert!(image_source_admissible(true, true, false));
        assert!(
            !image_source_admissible(false, true, false),
            "桌面不看新鲜度"
        );
        assert!(!image_source_admissible(false, false, false));
        // 移动端：信任表恒空 → 新鲜观察放行
        assert!(image_source_admissible(false, true, true));
        assert!(!image_source_admissible(false, false, true));
        assert!(image_source_admissible(true, false, true));
    }

    #[test]
    fn clipboard_freshness_window() {
        assert!(clipboard_peer_fresh(0, 299_999));
        assert!(!clipboard_peer_fresh(0, 300_000), "5 分钟窗口");
        // 时钟回拨不 panic
        assert!(clipboard_peer_fresh(1000, 0));
    }
}
