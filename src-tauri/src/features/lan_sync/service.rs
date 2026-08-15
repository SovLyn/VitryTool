//! lan-sync 业务核心（纯逻辑，无 IO，可脱离 Tauri 上下文独立测试）。
//!
//! 领域语义与行为契约见 `docs/api/lan-sync.md` 第 5 节；
//! 决策记录见 `dev/interface-drafts/lan-sync-contract-draft.md`。
//!
//! 包含：设置（开关/终端名）、跨端信封（Envelope）、内容指纹、收件箱分桶逻辑。

use serde::{Deserialize, Serialize};

/// 跨端协议版本（与项目版本同步；兼容约束见契约 5.6）。
pub const PROTOCOL_VERSION: &str = "0.2.5";
/// 固定 gossipsub 主题名（契约 5.6）。
pub const TOPIC: &str = "vitrytool-lan-clipboard";
/// 每来源节点桶内最多条目数（契约 5.3）。
pub const MAX_ENTRIES_PER_NODE: usize = 8;
/// 全局最多节点桶数（契约 5.3）。
pub const MAX_NODES: usize = 8;
/// 防环「近期接收指纹」LRU 上限（契约 5.4）。
pub const MAX_RECEIVED_FINGERPRINTS: usize = 100;
/// 广播消息体积上限（1MiB，契约 5.2 / 5.6，README 已知限制）。
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// 终端名长度上限（契约第 4 节）。
pub const TERMINAL_NAME_MAX_LEN: usize = 32;

/// `getLanSyncStatus` 响应（契约第 3 节）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanSyncStatus {
    /// 本机节点身份（完整 peerId）。
    pub peer_id: String,
    /// 终端名（默认主机名）。
    pub terminal_name: String,
    pub broadcast_enabled: bool,
    pub receive_enabled: bool,
    /// 节点是否在运行。
    pub node_running: bool,
    /// 当前已连接/发现的终端数。
    pub peer_count: usize,
}

// ---------------------------------------------------------------------------
// 设置
// ---------------------------------------------------------------------------

/// 同步设置（持久化于 AppData/lan-sync.json，见 store.rs）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanSettings {
    /// 广播开关（默认开）。
    pub broadcast_enabled: bool,
    /// 接收开关（默认开）。
    pub receive_enabled: bool,
    /// 终端名（默认主机名；由初始化流程填充）。
    pub terminal_name: String,
}

impl Default for LanSettings {
    fn default() -> Self {
        Self {
            broadcast_enabled: true,
            receive_enabled: true,
            terminal_name: String::new(),
        }
    }
}

/// 终端名校验：非空、长度 ≤ 32、不含控制字符。
pub fn validate_terminal_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= TERMINAL_NAME_MAX_LEN
        && !name.chars().any(|c| c.is_control())
}

// ---------------------------------------------------------------------------
// 跨端信封（契约 5.6）
// ---------------------------------------------------------------------------

/// 图片元数据（首版仅元数据；字节传输 TODO）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// 广播信封（gossipsub 载荷，JSON 序列化）。
///
/// 兼容约束：**只增字段**；接收端用 serde default 解析未知/缺失字段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub v: String,
    /// 发送方 unix 毫秒。
    pub ts: u128,
    /// 发送方 peerId（身份，非 IP）。
    pub peer_id: String,
    /// 发送方终端名快照。
    pub terminal: String,
    /// 携带格式声明（text/html/rtf/files/image）。
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_meta: Option<ImageMeta>,
}

// ---------------------------------------------------------------------------
// 内容指纹（契约 5.4；与本地历史同款规则：text → html → rtf → 文件 → 图片名）
// ---------------------------------------------------------------------------

/// 计算内容指纹；无任何内容返回 None。
pub fn fingerprint_of(
    text: Option<&str>,
    html: Option<&str>,
    rtf: Option<&str>,
    file_paths: Option<&[String]>,
    image_name: Option<&str>,
) -> Option<String> {
    if let Some(t) = text.filter(|t| !t.is_empty()) {
        return Some(t.to_string());
    }
    if let Some(h) = html.filter(|h| !h.is_empty()) {
        return Some(h.to_string());
    }
    if let Some(r) = rtf.filter(|r| !r.is_empty()) {
        return Some(r.to_string());
    }
    if let Some(f) = file_paths.filter(|f| !f.is_empty()) {
        return Some(f.join("\n"));
    }
    image_name
        .filter(|n| !n.is_empty())
        .map(|n| format!("[image] {n}"))
}

/// 计算信封指纹（接收侧与广播侧共用同一规则）。
pub fn envelope_fingerprint(env: &Envelope) -> Option<String> {
    fingerprint_of(
        env.text.as_deref(),
        env.html.as_deref(),
        env.rtf.as_deref(),
        env.file_paths.as_deref(),
        env.image_meta.as_ref().map(|m| m.name.as_str()),
    )
}

// ---------------------------------------------------------------------------
// 收件箱（契约 5.3）
// ---------------------------------------------------------------------------

/// 收件箱条目（响应结构直接序列化给前端）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InboxEntry {
    pub id: String,
    pub peer_id: String,
    pub terminal_name: String,
    /// 本机接收时间（ISO 8601，排序键）。
    pub received_at: String,
    /// 发送方时间（ISO 8601，展示用）。
    pub sent_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_meta: Option<ImageMeta>,
    /// 去重键（与广播指纹同规则）。
    pub fingerprint: String,
}

/// 一个来源节点的桶（entries[0] 为最新）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InboxNode {
    pub peer_id: String,
    /// 桶内最新条目的终端名快照（可能随新消息变化）。
    pub terminal_name: String,
    pub entries: Vec<InboxEntry>,
}

/// 收件箱（nodes 按桶内最新 receivedAt 倒序）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct InboxData {
    pub nodes: Vec<InboxNode>,
}

/// 插入结果分类。
#[derive(Debug, Clone, PartialEq)]
pub enum InboxOutcome {
    /// 新条目入桶。
    New,
    /// 指纹命中既有条目，刷新置顶（不新增）。
    DedupPromoted,
    /// 新节点触发全局淘汰（契约 5.3：淘汰「桶内最新条目最旧」的节点整桶）。
    NodeEvicted { evicted_peer_id: String },
}

fn received_at_key(entry: &InboxEntry) -> &str {
    &entry.received_at
}

/// 维持不变量：nodes 按桶内最新条目 receivedAt 倒序。
fn resort(data: &mut InboxData) {
    data.nodes.sort_by(|a, b| {
        let a_newest = a.entries.first().map(received_at_key).unwrap_or_default();
        let b_newest = b.entries.first().map(received_at_key).unwrap_or_default();
        b_newest.cmp(a_newest)
    });
}

/// 插入一条收到的消息（去重置顶 / 每桶 8 条 / 全局 8 桶淘汰，契约 5.3）。
pub fn insert_message(data: &mut InboxData, entry: InboxEntry) -> InboxOutcome {
    let peer_id = entry.peer_id.clone();

    if let Some(node) = data.nodes.iter_mut().find(|n| n.peer_id == peer_id) {
        // 去重置顶：命中同指纹 → 移出旧条目置顶（保留原 id，刷新终端名快照）
        if let Some(pos) = node
            .entries
            .iter()
            .position(|e| e.fingerprint == entry.fingerprint)
        {
            let mut existing = node.entries.remove(pos);
            // 去重置顶：保留原 id 与内容，刷新接收时间与终端名快照（契约 5.3）
            existing.received_at = entry.received_at.clone();
            existing.terminal_name = entry.terminal_name.clone();
            node.entries.insert(0, existing);
            resort(data);
            return InboxOutcome::DedupPromoted;
        }
        // 新条目：插入头部，超限截断（淘汰该桶最旧）
        node.entries.insert(0, entry);
        node.entries.truncate(MAX_ENTRIES_PER_NODE);
        resort(data);
        return InboxOutcome::New;
    }

    // 新节点：先入桶再检查全局上限（契约 5.3：淘汰「桶内最新条目最旧」的节点整桶；
    // 刚插入的自身时间最新，不会被淘汰）
    data.nodes.push(InboxNode {
        peer_id: peer_id.clone(),
        terminal_name: entry.terminal_name.clone(),
        entries: vec![entry],
    });
    if data.nodes.len() > MAX_NODES {
        let oldest_idx = data
            .nodes
            .iter()
            .enumerate()
            .min_by_key(|(_, n)| n.entries.first().map(received_at_key).unwrap_or_default())
            .map(|(i, _)| i);
        if let Some(idx) = oldest_idx {
            let evicted_peer_id = data.nodes.remove(idx).peer_id;
            resort(data);
            return InboxOutcome::NodeEvicted { evicted_peer_id };
        }
    }
    resort(data);
    InboxOutcome::New
}

/// 单条删除（契约 5.3 / 命令 deleteLanInboxEntry）。
pub fn delete_entry(data: &mut InboxData, id: &str) -> bool {
    for node in &mut data.nodes {
        if let Some(pos) = node.entries.iter().position(|e| e.id == id) {
            node.entries.remove(pos);
            return true;
        }
    }
    false
}

/// 清空（命令 clearLanInbox）。
pub fn clear_inbox(data: &mut InboxData) {
    data.nodes.clear();
}

/// 按展示结构取收件箱（契约 3）：nodes 按桶内最新条目倒序，桶内条目按 receivedAt 倒序。
pub fn inbox_for_display(data: &InboxData) -> InboxData {
    let mut out = data.clone();
    resort(&mut out);
    out
}

// ---------------------------------------------------------------------------
// 构建广播信封
// ---------------------------------------------------------------------------

/// 从剪贴板历史条目（序列化 JSON）构建信封。
///
/// 字段约定与 `ClipboardEntry` 的 camelCase 序列化一致。
pub fn envelope_from_entry_json(
    value: &serde_json::Value,
    self_peer_id: &str,
    terminal_name: &str,
    now_ms: u128,
) -> Option<Envelope> {
    let text = value.get("text").and_then(|v| v.as_str()).map(String::from);
    let html = value.get("html").and_then(|v| v.as_str()).map(String::from);
    let rtf = value.get("rtf").and_then(|v| v.as_str()).map(String::from);
    let file_paths = value
        .get("files")
        .and_then(|f| f.get("paths"))
        .and_then(|p| p.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
        .filter(|v| !v.is_empty());
    let image_meta = value.get("image").and_then(|img| {
        let path = img.get("path")?.as_str()?;
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        Some(ImageMeta {
            name,
            width: img.get("width").and_then(|v| v.as_u64()).map(|v| v as u32),
            height: img.get("height").and_then(|v| v.as_u64()).map(|v| v as u32),
            size: img.get("size").and_then(|v| v.as_u64()),
        })
    });

    let kinds: Vec<String> = [
        ("text", text.is_some()),
        ("html", html.is_some()),
        ("rtf", rtf.is_some()),
        ("files", file_paths.is_some()),
        ("image", image_meta.is_some()),
    ]
    .iter()
    .filter(|(_, has)| *has)
    .map(|(kind, _)| kind.to_string())
    .collect();

    if kinds.is_empty() {
        return None;
    }

    Some(Envelope {
        v: PROTOCOL_VERSION.to_string(),
        ts: now_ms,
        peer_id: self_peer_id.to_string(),
        terminal: terminal_name.to_string(),
        kinds,
        text,
        html,
        rtf,
        file_paths,
        image_meta,
    })
}

/// 从信封构建收件箱条目（接收侧）。
pub fn inbox_entry_from_envelope(
    env: &Envelope,
    received_at_iso: String,
) -> Option<InboxEntry> {
    let fingerprint = envelope_fingerprint(env)?;
    Some(InboxEntry {
        id: uuid::Uuid::new_v4().to_string(),
        peer_id: env.peer_id.clone(),
        terminal_name: env.terminal.clone(),
        received_at: received_at_iso,
        sent_at: iso_from_ms(env.ts),
        text: env.text.clone(),
        html: env.html.clone(),
        rtf: env.rtf.clone(),
        file_paths: env.file_paths.clone(),
        image_meta: env.image_meta.clone(),
        fingerprint,
    })
}

/// unix 毫秒 → ISO 8601（UTC）。
pub fn iso_from_ms(ms: u128) -> String {
    let secs = (ms / 1000) as i64;
    let nanos = ((ms % 1000) as u32) * 1_000_000;
    chrono::DateTime::from_timestamp(secs, nanos)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[cfg(test)]
mod pure_tests {
    use super::*;

    fn env_with(text: &str) -> Envelope {
        Envelope {
            v: PROTOCOL_VERSION.into(),
            ts: 1000,
            peer_id: "peer-a".into(),
            terminal: "A".into(),
            kinds: vec!["text".into()],
            text: Some(text.into()),
            html: None,
            rtf: None,
            file_paths: None,
            image_meta: None,
        }
    }

    fn inbox_entry(peer: &str, fp: &str, recv: &str) -> InboxEntry {
        InboxEntry {
            id: uuid::Uuid::new_v4().to_string(),
            peer_id: peer.into(),
            terminal_name: peer.into(),
            received_at: recv.into(),
            sent_at: recv.into(),
            text: Some(fp.into()),
            html: None,
            rtf: None,
            file_paths: None,
            image_meta: None,
            fingerprint: fp.into(),
        }
    }

    #[test]
    fn fingerprint_prefers_text() {
        assert_eq!(
            fingerprint_of(Some("t"), Some("h"), None, None, None),
            Some("t".into())
        );
    }

    #[test]
    fn fingerprint_falls_back_html_rtf_files_image() {
        assert_eq!(fingerprint_of(None, Some("h"), None, None, None), Some("h".into()));
        assert_eq!(fingerprint_of(None, None, Some("r"), None, None), Some("r".into()));
        assert_eq!(
            fingerprint_of(None, None, None, Some(&["a".into(), "b".into()]), None),
            Some("a\nb".into())
        );
        assert_eq!(
            fingerprint_of(None, None, None, None, Some("x.png")),
            Some("[image] x.png".into())
        );
        assert_eq!(fingerprint_of(None, None, None, None, None), None);
    }

    #[test]
    fn envelope_fingerprint_roundtrip() {
        let env = env_with("hello");
        assert_eq!(envelope_fingerprint(&env), Some("hello".into()));
    }

    #[test]
    fn validate_terminal_name_rules() {
        assert!(validate_terminal_name("SOVLYN"));
        assert!(!validate_terminal_name(""));
        assert!(!validate_terminal_name("   "));
        assert!(!validate_terminal_name(&"x".repeat(33)));
        assert!(!validate_terminal_name("a\nb"));
    }

    #[test]
    fn insert_new_entry_and_order_nodes() {
        let mut data = InboxData::default();
        assert_eq!(
            insert_message(&mut data, inbox_entry("p1", "f1", "2026-08-14T10:00:00Z")),
            InboxOutcome::New
        );
        insert_message(&mut data, inbox_entry("p2", "f2", "2026-08-14T10:00:01Z"));
        // nodes 按最新条目倒序：p2 在前
        assert_eq!(data.nodes[0].peer_id, "p2");
        assert_eq!(data.nodes[1].peer_id, "p1");
    }

    #[test]
    fn dedup_promotes_existing_entry() {
        let mut data = InboxData::default();
        insert_message(&mut data, inbox_entry("p1", "same", "2026-08-14T10:00:00Z"));
        insert_message(&mut data, inbox_entry("p2", "other", "2026-08-14T10:00:01Z"));
        // p1 再发同指纹 → 去重置顶，不新增，节点 p1 升到最前
        assert_eq!(
            insert_message(&mut data, inbox_entry("p1", "same", "2026-08-14T10:00:02Z")),
            InboxOutcome::DedupPromoted
        );
        assert_eq!(data.nodes[0].peer_id, "p1");
        assert_eq!(data.nodes[0].entries.len(), 1);
        assert_eq!(data.nodes[0].entries[0].received_at, "2026-08-14T10:00:02Z");
    }

    #[test]
    fn per_node_caps_at_eight() {
        let mut data = InboxData::default();
        for i in 0..10 {
            insert_message(
                &mut data,
                inbox_entry("p1", &format!("f{i}"), &format!("2026-08-14T10:00:{i:02}Z")),
            );
        }
        assert_eq!(data.nodes[0].entries.len(), MAX_ENTRIES_PER_NODE);
        // 最新 8 条：f9..f2（f0/f1 被淘汰）
        assert_eq!(data.nodes[0].entries[0].fingerprint, "f9");
        assert_eq!(data.nodes[0].entries[7].fingerprint, "f2");
    }

    #[test]
    fn global_evicts_least_active_node() {
        let mut data = InboxData::default();
        // 8 个节点，各 1 条；p0 最旧
        for i in 0..MAX_NODES {
            insert_message(
                &mut data,
                inbox_entry(
                    &format!("p{i}"),
                    &format!("f{i}"),
                    &format!("2026-08-14T10:00:{i:02}Z"),
                ),
            );
        }
        assert_eq!(data.nodes.len(), MAX_NODES);
        // 第 9 个节点 p8（最新）→ 淘汰 p0（桶内最新条目最旧）
        let outcome = insert_message(
            &mut data,
            inbox_entry("p8", "f8", "2026-08-14T10:00:08Z"),
        );
        assert_eq!(
            outcome,
            InboxOutcome::NodeEvicted {
                evicted_peer_id: "p0".into()
            }
        );
        assert_eq!(data.nodes.len(), MAX_NODES);
        assert!(!data.nodes.iter().any(|n| n.peer_id == "p0"));
        assert!(data.nodes.iter().any(|n| n.peer_id == "p8"));
        assert_eq!(data.nodes[0].peer_id, "p8");
    }

    #[test]
    fn delete_and_clear() {
        let mut data = InboxData::default();
        let e = inbox_entry("p1", "f1", "2026-08-14T10:00:00Z");
        let id = e.id.clone();
        insert_message(&mut data, e);
        assert!(delete_entry(&mut data, &id));
        assert!(!delete_entry(&mut data, &id));
        insert_message(&mut data, inbox_entry("p2", "f2", "2026-08-14T10:00:01Z"));
        clear_inbox(&mut data);
        assert!(data.nodes.is_empty());
    }

    #[test]
    fn envelope_from_entry_json_maps_fields() {
        let entry = serde_json::json!({
            "text": "hello",
            "html": "<p>hello</p>",
            "image": { "path": "C:\\x\\a.png", "width": 10, "height": 20, "size": 100 }
        });
        let env = envelope_from_entry_json(&entry, "self", "ME", 1234).unwrap();
        assert_eq!(env.peer_id, "self");
        assert_eq!(env.terminal, "ME");
        assert_eq!(env.v, PROTOCOL_VERSION);
        assert_eq!(env.kinds, vec!["text", "html", "image"]);
        assert_eq!(env.image_meta.as_ref().unwrap().name, "a.png");
        assert_eq!(env.ts, 1234);
    }

    #[test]
    fn inbox_entry_from_envelope_uses_fingerprint() {
        let env = env_with("hello");
        let entry = inbox_entry_from_envelope(&env, "2026-08-14T10:00:00Z".into()).unwrap();
        assert_eq!(entry.peer_id, "peer-a");
        assert_eq!(entry.fingerprint, "hello");
        assert_eq!(entry.sent_at, "1970-01-01T00:00:01Z");
    }

    #[test]
    fn iso_from_ms_converts() {
        assert_eq!(iso_from_ms(1000), "1970-01-01T00:00:01Z");
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00Z");
    }
}
