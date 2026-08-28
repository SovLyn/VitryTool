//! lan-file 持久化与共享数据结构（契约 `docs/api/lan-file.md` 5.1 / 5.5）。
//!
//! - `AppData/lan-file.json`：总开关 + TOFU 信任表（经 tauri-plugin-store，键 `settings`）；
//! - `AppData/lanfile/`：接收落盘目录；
//! - `AppData/lanfile/.<transferId>.meta.json`：续传 sidecar（含 `cancelled` 墓碑）；
//! - `AppData/lan-inbox-images/`：自动图片通道落盘目录（LRU 200 张 / 500MB）。
//!
//! 注：类型与工具由 `state.rs` 消费；阶段性未消费告警以模块级 allow 抑制，接线后移除。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;

// ---------------------------------------------------------------------------
// 设置与信任表（lan-file.json）
// ---------------------------------------------------------------------------

/// 已信任终端（TOFU 表项；表按 peerId 键控，同名两条记录可并存，契约 5.4）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedPeer {
    pub peer_id: String,
    pub terminal_name: String,
    /// 信任时刻 ISO 8601。
    pub trusted_at: String,
}

/// lan-file 设置（持久化于 AppData/lan-file.json）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanFileSettings {
    /// 总开关（默认开，契约 5.1）。
    pub enabled: bool,
    pub trusted_peers: Vec<TrustedPeer>,
}

impl Default for LanFileSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            trusted_peers: Vec::new(),
        }
    }
}

impl LanFileSettings {
    /// peerId 是否已信任。
    pub fn is_trusted(&self, peer_id: &str) -> bool {
        self.trusted_peers.iter().any(|p| p.peer_id == peer_id)
    }

    /// 写入信任（TOFU 确认；已存在则刷新终端名与时间）。
    pub fn trust(&mut self, peer_id: &str, terminal_name: &str, trusted_at: String) {
        if let Some(existing) = self.trusted_peers.iter_mut().find(|p| p.peer_id == peer_id) {
            existing.terminal_name = terminal_name.to_string();
            existing.trusted_at = trusted_at;
            return;
        }
        self.trusted_peers.push(TrustedPeer {
            peer_id: peer_id.to_string(),
            terminal_name: terminal_name.to_string(),
            trusted_at,
        });
    }

    /// 移除信任（契约 5.4：下次该终端提议重新走 TOFU 弹窗）。返回是否找到。
    pub fn untrust(&mut self, peer_id: &str) -> bool {
        let before = self.trusted_peers.len();
        self.trusted_peers.retain(|p| p.peer_id != peer_id);
        self.trusted_peers.len() != before
    }

    /// 撞名判定（契约 5.4 复审修订 A）：陌生 peerId 的终端名撞已信任项。
    pub fn name_clash(&self, peer_id: &str, terminal_name: &str) -> bool {
        !self.is_trusted(peer_id)
            && self
                .trusted_peers
                .iter()
                .any(|p| p.terminal_name == terminal_name)
    }
}

// ---------------------------------------------------------------------------
// 续传 sidecar（AppData/lanfile/.<transferId>.meta.json，契约 5.5）
// ---------------------------------------------------------------------------

/// sidecar 内单文件进度。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SidecarFile {
    pub name: String,
    /// `.tmp` 文件名（非路径）。
    pub tmp_name: String,
    pub received_bytes: u64,
}

/// 续传 sidecar：任务生命周期实例的落盘进度与取消墓碑。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SidecarMeta {
    pub transfer_id: String,
    pub direction: String, // "receive"
    pub peer_id: String,
    pub files: Vec<SidecarFile>,
    /// 取消墓碑（契约 5.5：用户显式取消 → 永久终止，不再续传）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<bool>,
}

impl SidecarMeta {
    /// 是否已写取消墓碑（墓碑在 → 一律拒绝续传，契约 5.5）。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled == Some(true)
    }
}

// ---------------------------------------------------------------------------
// 纯逻辑工具（dt 覆盖点）
// ---------------------------------------------------------------------------

/// 文件名净化（契约 5.5 落盘安全）：去路径分隔符 / 控制字符 / 盘符，防目录穿越。
///
/// - 仅取最后一段（剥所有父目录与盘符）；
/// - 删除 `/` `\` `:` 与控制字符；
/// - 空名或全 `.` → `unnamed`；超长截断（保留扩展名 255 字符内）。
pub fn sanitize_filename(name: &str) -> String {
    // 剥盘符（如 "C:"）与路径分隔符：取分隔符后的最后一段
    let last = name
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let last = last.rsplit(':').next().unwrap_or(last);
    let cleaned: String = last.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        return "unnamed".into();
    }
    // 预留扩展名截断
    if cleaned.chars().count() > 255 {
        let ext = Path::new(&cleaned)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let stem: String = cleaned.chars().take(255 - ext.chars().count()).collect();
        return format!("{stem}{ext}");
    }
    cleaned
}

/// 重名自动改名（契约 5.5）：`name (1).ext`、`name (2).ext`…
///
/// `taken` 为目录中已存在（或已规划占用）的名字集合。
pub fn dedup_filename(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let path = Path::new(name);
    let (stem, ext) = match path.extension() {
        Some(ext) => (
            path.with_extension("").to_string_lossy().into_owned(),
            format!(".{}", ext.to_string_lossy()),
        ),
        None => (name.to_string(), String::new()),
    };
    for i in 1..=u32::MAX {
        let candidate = format!("{stem} ({i}){ext}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    // 理论不可达；防御性兜底
    format!("{stem}-{name}{ext}")
}

/// 磁盘空间检查（契约 5.5）：剩余 ≥ total + 200MB。
pub fn disk_space_ok(available_bytes: u64, total_bytes: u64) -> bool {
    const MARGIN: u64 = 200 * 1024 * 1024;
    available_bytes >= total_bytes.saturating_add(MARGIN)
}

/// 全文件 SHA-256（hex；落盘后重读对账用）。
pub fn file_sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

/// 字节 hex 编码（小写）。
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 图片扩展名白名单（契约 5.7-2）。
pub const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// 扩展名是否在图片白名单（大小写不敏感）。
pub fn is_image_ext(name: &str) -> bool {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .map(|e| IMAGE_EXTS.contains(&e.as_str()))
        .unwrap_or(false)
}

/// 自动图片通道门槛（契约 5.7）：≤10MiB + 白名单扩展名 + hash 匹配。
pub fn image_offer_admissible(size: u64, name: &str, offer_hash: &str, meta_hash: &str) -> bool {
    const MAX: u64 = 10 * 1024 * 1024;
    size <= MAX && is_image_ext(name) && offer_hash == meta_hash
}

/// 图片 LRU 上限（契约 5.7-3）。
pub const IMAGE_LRU_MAX_FILES: usize = 200;
/// 图片 LRU 字节上限。
pub const IMAGE_LRU_MAX_BYTES: u64 = 500 * 1024 * 1024;

/// 计算需要删除的图片（超 LRU 上限）：按 mtime 从旧到新删，直到满足双上限。
pub fn lru_evict_images(files: &[(PathBuf, std::time::SystemTime, u64)]) -> Vec<PathBuf> {
    let mut files = files.to_vec();
    // 旧在前
    files.sort_by_key(|(_, mtime, _)| *mtime);
    let mut total: u64 = files.iter().map(|(_, _, s)| s).sum();
    let mut evict = Vec::new();
    let mut keep_count = files.len();
    for (idx, (path, _, size)) in files.iter().enumerate() {
        if keep_count <= IMAGE_LRU_MAX_FILES && total <= IMAGE_LRU_MAX_BYTES {
            break;
        }
        evict.push(path.clone());
        total = total.saturating_sub(*size);
        keep_count -= 1;
        let _ = idx;
    }
    evict
}

// ---------------------------------------------------------------------------
// tauri-plugin-store 实现（lan-file.json）
// ---------------------------------------------------------------------------

/// 存储错误（state 层转 ApiError / String）。
#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError(e.to_string())
    }
}

/// 基于 tauri-plugin-store 的 lan-file 设置持久化（AppData/lan-file.json，键 `settings`）。
pub struct StoreBackend {
    store: std::sync::Arc<tauri_plugin_store::Store<tauri::Wry>>,
}

impl StoreBackend {
    pub fn new(app: &AppHandle) -> Result<Self, StoreError> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| StoreError(format!("app data dir: {e}")))?;
        let store = app
            .store(dir.join("lan-file.json"))
            .map_err(|e| StoreError(format!("open lan-file.json: {e}")))?;
        Ok(Self { store })
    }

    pub fn load_settings(&self) -> Result<LanFileSettings, StoreError> {
        match self.store.get("settings") {
            Some(v) => {
                serde_json::from_value(v).map_err(|e| StoreError(format!("parse settings: {e}")))
            }
            None => Ok(LanFileSettings::default()),
        }
    }

    pub fn save_settings(&self, settings: &LanFileSettings) -> Result<(), StoreError> {
        let v = serde_json::to_value(settings)
            .map_err(|e| StoreError(format!("serialize settings: {e}")))?;
        self.store.set("settings", v);
        self.store
            .save()
            .map_err(|e| StoreError(format!("save lan-file.json: {e}")))
    }
}

/// 读取 sidecar；不存在 → Ok(None)。
pub fn load_sidecar(dir: &Path, transfer_id: &str) -> std::io::Result<Option<SidecarMeta>> {
    let path = dir.join(format!(".{transfer_id}.meta.json"));
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)?;
    match serde_json::from_slice::<SidecarMeta>(&bytes) {
        Ok(meta) => Ok(Some(meta)),
        Err(e) => {
            log::warn!("lan_file: corrupt sidecar {}: {e}", path.display());
            Ok(None)
        }
    }
}

/// 写 sidecar（原子性非硬要求：进度文件，损坏可丢续传）。
pub fn save_sidecar(dir: &Path, meta: &SidecarMeta) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!(".{}.meta.json", meta.transfer_id));
    let bytes = serde_json::to_vec(meta).map_err(std::io::Error::other)?;
    fs::write(&path, bytes)
}

/// 删除 sidecar。
pub fn remove_sidecar(dir: &Path, transfer_id: &str) -> std::io::Result<()> {
    let path = dir.join(format!(".{transfer_id}.meta.json"));
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

/// 清理 `.tmp` 文件（取消 / 完整性失败 / 超窗）。
pub fn remove_tmp_file(dir: &Path, tmp_name: &str) {
    let path = dir.join(tmp_name);
    if path.exists() {
        if let Err(e) = fs::remove_file(&path) {
            log::debug!("lan_file: remove tmp {} failed: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_enabled_and_empty_trust() {
        let s = LanFileSettings::default();
        assert!(s.enabled);
        assert!(s.trusted_peers.is_empty());
        // serde 形状（camelCase）
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["enabled"], true);
        assert!(json.get("trustedPeers").is_some());
    }

    #[test]
    fn trust_untrust_and_name_clash() {
        let mut s = LanFileSettings::default();
        s.trust("peerA", "SOVLYN", "2026-08-20T00:00:00Z".into());
        assert!(s.is_trusted("peerA"));
        assert!(!s.name_clash("peerA", "SOVLYN"), "已信任不判撞名");
        assert!(!s.name_clash("peerB", "OTHER"), "无撞名");
        assert!(s.name_clash("peerB", "SOVLYN"), "陌生 peerId 撞名");
        // 重复信任刷新
        s.trust("peerA", "RENAMED", "2026-08-21T00:00:00Z".into());
        assert_eq!(s.trusted_peers.len(), 1);
        assert_eq!(s.trusted_peers[0].terminal_name, "RENAMED");
        // 同名两条记录并存（peerId 键控）
        s.trust("peerB", "SOVLYN", "2026-08-21T00:00:00Z".into());
        assert_eq!(s.trusted_peers.len(), 2);
        // 移除
        assert!(s.untrust("peerA"));
        assert!(!s.untrust("peerA"));
        assert_eq!(s.trusted_peers.len(), 1);
    }

    #[test]
    fn sanitize_strips_paths_controls_and_drive() {
        assert_eq!(sanitize_filename("a.txt"), "a.txt");
        assert_eq!(sanitize_filename(r"C:\evil\path\a.txt"), "a.txt");
        assert_eq!(sanitize_filename("/etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("../../..\\..\\x/a.txt"), "a.txt");
        assert_eq!(sanitize_filename("C:evil"), "evil");
        // 控制字符剔除
        assert_eq!(sanitize_filename("a\u{0}b\u{7}.txt"), "ab.txt");
        // 空与全点
        assert_eq!(sanitize_filename(""), "unnamed");
        assert_eq!(sanitize_filename("   "), "unnamed");
        assert_eq!(sanitize_filename(".."), "unnamed");
        assert_eq!(sanitize_filename("..."), "unnamed");
        // 超长截断保留扩展名
        let long = format!("{}.txt", "x".repeat(300));
        let sanitized = sanitize_filename(&long);
        assert!(sanitized.chars().count() <= 255);
        assert!(sanitized.ends_with(".txt"));
        // 目录穿越残留段
        assert_eq!(sanitize_filename("..\\..\\boot.ini"), "boot.ini");
    }

    #[test]
    fn dedup_filename_increments() {
        let existing = vec!["a.txt", "a (1).txt"];
        let taken = |n: &str| existing.contains(&n);
        assert_eq!(dedup_filename("b.txt", &taken), "b.txt");
        assert_eq!(dedup_filename("a.txt", &taken), "a (2).txt");
        let existing2 = vec!["noext"];
        let taken2 = |n: &str| existing2.contains(&n);
        assert_eq!(dedup_filename("noext", &taken2), "noext (1)");
    }

    #[test]
    fn disk_space_check_margin() {
        let need = 1024u64;
        assert!(!disk_space_ok(need, need), "不足 200MB 余量");
        assert!(disk_space_ok(need + 200 * 1024 * 1024, need));
        assert!(disk_space_ok(u64::MAX, need));
    }

    #[test]
    fn image_gates() {
        assert!(is_image_ext("a.PNG"));
        assert!(is_image_ext("b.jpeg"));
        assert!(!is_image_ext("c.exe"));
        assert!(!is_image_ext("d"));
        // 10MiB 门槛
        let max = 10 * 1024 * 1024u64;
        assert!(image_offer_admissible(max, "a.png", "h1", "h1"));
        assert!(!image_offer_admissible(max + 1, "a.png", "h1", "h1"));
        assert!(!image_offer_admissible(1, "a.exe", "h1", "h1"));
        assert!(!image_offer_admissible(1, "a.png", "h1", "h2"));
    }

    #[test]
    fn lru_evicts_oldest_until_within_limits() {
        let mk = |i: u64, size: u64| {
            (
                PathBuf::from(format!("img{i}.png")),
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(i),
                size,
            )
        };
        let files: Vec<_> = (0..5).map(|i| mk(i, 100)).collect();
        let evict = lru_evict_images(&files);
        assert!(evict.is_empty(), "未超限不删");

        let mut files: Vec<_> = (0..5).map(|i| mk(i, 100)).collect();
        files.push(mk(10, 100));
        // 上限 200 永不触发；用字节上限行为验证（以常数缩放）——此处直接构造 3 项
        // 500MB 上限对单测过大，验证删除顺序逻辑：构造已超「文件数」上限的场景需 201 项，
        // 代价可接受
        let big: Vec<_> = (0..201).map(|i| mk(i, 1)).collect();
        let evict = lru_evict_images(&big);
        assert_eq!(evict.len(), 1);
        assert_eq!(evict[0], PathBuf::from("img0.png"));
        let _ = &mut files;
    }

    #[test]
    fn hex_and_sha_helpers() {
        assert_eq!(hex_encode(&[0xde, 0xad]), "dead");
        let dir = std::env::temp_dir().join(format!("vitry-sha-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.bin");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(
            file_sha256_hex(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
