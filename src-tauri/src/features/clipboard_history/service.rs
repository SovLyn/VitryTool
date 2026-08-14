//! 剪贴板历史业务核心（纯逻辑，无 IO，可脱离 Tauri 上下文独立测试）。
//!
//! 领域术语与规则见 `dev/CONTEXT.md`；行为契约见 `docs/api/clipboard-history.md` 第 5 节。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 条目上限（允许的最大值）。
pub const MAX_ENTRIES_LIMIT: usize = 1024;
/// 默认条目上限。
pub const DEFAULT_MAX_ENTRIES: usize = 64;

/// 一条剪贴板历史记录（一次剪贴板变化 = 一条，各格式字段共存）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardEntry {
    /// 唯一标识（UUID）。
    pub id: String,
    /// ISO 8601，捕捉时刻（后端取系统时间）。
    pub captured_at: String,
    /// 收藏时刻（ISO 8601）；存在即收藏（契约 5.8、`dev/CONTEXT.md`「收藏」）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorited_at: Option<String>,
    /// 纯文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 原始 HTML（保真）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// 原始 RTF（保真）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtf: Option<String>,
    /// 图片（本体由插件落盘，此处仅引用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ClipboardImage>,
    /// 文件引用（不复制本体）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<ClipboardFiles>,
}

impl ClipboardEntry {
    /// 是否不含任何内容字段（捕捉时无可用内容则静默忽略，见契约 5.2-2）。
    pub fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.html.is_none()
            && self.rtf.is_none()
            && self.image.is_none()
            && self.files.is_none()
    }

    /// 是否已收藏（`favorited_at` 存在即收藏，见契约 5.8）。
    pub fn is_favorite(&self) -> bool {
        self.favorited_at.is_some()
    }
}

/// 图片引用（本体在插件默认目录，`missing` 为派生标记、不持久化）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardImage {
    /// 插件保存的 .png 绝对路径。
    pub path: String,
    /// 字节数。
    pub size: u64,
    /// 像素宽。
    pub width: u32,
    /// 像素高。
    pub height: u32,
    /// 派生：文件是否已不存在（store 持久化时忽略）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub missing: bool,
}

/// 文件引用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardFiles {
    /// 源文件绝对路径。
    pub paths: Vec<String>,
    /// 总字节数。
    pub size: u64,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// `cleanupOrphanImages` 响应：删除的孤儿图片数。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResp {
    pub removed: u32,
}

/// `setMaxEntries` 响应：生效值 + 因截断删除的条目数。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SetMaxResp {
    pub max_entries: u32,
    pub evicted: u32,
}

/// 内容指纹匹配判定（契约 5.2-4）：文本按内容，其次图片按落盘路径，再其次纯富文本按 HTML/RTF。
pub fn fingerprint_matches(a: &ClipboardEntry, b: &ClipboardEntry) -> bool {
    if let (Some(a_text), Some(b_text)) = (&a.text, &b.text) {
        return a_text == b_text;
    }
    if let (Some(a_img), Some(b_img)) = (&a.image, &b.image) {
        return a_img.path == b_img.path;
    }
    if let (Some(a_html), Some(b_html)) = (&a.html, &b.html) {
        return a_html == b_html;
    }
    if let (Some(a_rtf), Some(b_rtf)) = (&a.rtf, &b.rtf) {
        return a_rtf == b_rtf;
    }
    false
}

/// `captureClipboard` 的去重置顶 + 即时淘汰结果。
#[derive(Debug, Clone, PartialEq)]
pub struct InsertOutcome {
    /// 是否新增（false = 命中去重置顶）。
    pub is_new: bool,
    /// 新增或置顶后的条目。
    pub entry: ClipboardEntry,
    /// 因即时淘汰需要删除的图片文件路径（调用方负责删除）。
    pub evicted_files: Vec<PathBuf>,
}

/// 将 `incoming` 去重置顶或插入 `entries`（`entries[0]` 为最新），并即时淘汰超限条目。
///
/// 命中既有条目时仅刷新其 `captured_at` 并置顶（契约 5.2-4、D4），条数不变、无需淘汰。
pub fn dedup_promote_and_evict(
    entries: &mut Vec<ClipboardEntry>,
    incoming: ClipboardEntry,
    max_entries: usize,
) -> InsertOutcome {
    if let Some(pos) = entries
        .iter()
        .position(|existing| fingerprint_matches(existing, &incoming))
    {
        let mut existing = entries.remove(pos);
        existing.captured_at = incoming.captured_at.clone();
        entries.insert(0, existing.clone());
        return InsertOutcome {
            is_new: false,
            entry: existing,
            evicted_files: Vec::new(),
        };
    }

    entries.insert(0, incoming.clone());
    let evicted_files = evict_over_limit(entries, max_entries);
    InsertOutcome {
        is_new: true,
        entry: incoming,
        evicted_files,
    }
}

/// 展示排序（契约 5.8）：收藏区在前（区内按收藏时间倒序，最近收藏最前），
/// 其后普通条目按捕捉时间倒序。
///
/// 稳定排序：组内键值相同时保持原相对顺序（store 以捕捉时间倒序存储，普通区无需移动）。
pub fn sort_for_display(entries: &mut [ClipboardEntry]) {
    use std::cmp::Ordering;
    entries.sort_by(|a, b| match (a.is_favorite(), b.is_favorite()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) => b.favorited_at.cmp(&a.favorited_at),
        (false, false) => b.captured_at.cmp(&a.captured_at),
    });
}

/// 超过上限时删除最旧的非收藏条目（收藏豁免，契约 5.8），返回被删条目的图片文件路径。
///
/// 条目列表以捕捉时间倒序存储（最新在前），故最旧的非收藏条目 = 队尾方向第一个非收藏条目；
/// 若非收藏条目数 ≤ 上限（如全部为收藏），不淘汰任何条目。
pub fn evict_over_limit(entries: &mut Vec<ClipboardEntry>, max_entries: usize) -> Vec<PathBuf> {
    let mut evicted = Vec::new();
    while entries.iter().filter(|e| !e.is_favorite()).count() > max_entries {
        let Some(pos) = entries.iter().rposition(|e| !e.is_favorite()) else {
            break;
        };
        let removed = entries.remove(pos);
        if let Some(img) = removed.image {
            evicted.push(PathBuf::from(img.path));
        }
    }
    evicted
}

/// 设置/取消收藏（契约 5.8）：返回是否找到目标条目。
///
/// - 收藏：`favorited_at = now`（重复收藏刷新收藏时间，收藏区重新置顶，幂等）；
/// - 取消收藏：清空 `favorited_at`，不触发淘汰（容忍短暂超限，下次捕捉/调上限时归位）。
pub fn set_favorite(entries: &mut [ClipboardEntry], id: &str, favorited: bool, now: &str) -> bool {
    let Some(entry) = entries.iter_mut().find(|e| e.id == id) else {
        return false;
    };
    entry.favorited_at = if favorited {
        Some(now.to_string())
    } else {
        None
    };
    true
}

/// 计算孤儿图片：目录中存在但没有任何存活条目引用的文件（契约 5.4-②）。
///
/// 路径比较基于 [`Path::components`]（在 Windows 上 `/` 与 `\` 均被识别为路径分隔符），
/// 避免条目路径（插件用 `\` 拼接）与扫描目录（`PathBuf::join` 可能保留 `/`）
/// 因分隔符表示不一致而被误判为孤儿——该问题曾导致全部图片被误删。
pub fn orphan_files(entries: &[ClipboardEntry], dir_files: &[PathBuf]) -> Vec<PathBuf> {
    let referenced: Vec<Vec<_>> = entries
        .iter()
        .filter_map(|e| e.image.as_ref())
        .map(|img| Path::new(&img.path).components().collect())
        .collect();
    dir_files
        .iter()
        .filter(|file| {
            let comps = file.components().collect::<Vec<_>>();
            !referenced.contains(&comps)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod unit {
    use super::*;

    fn entry(id: &str, text: Option<&str>, image_path: Option<&str>) -> ClipboardEntry {
        ClipboardEntry {
            id: id.to_string(),
            captured_at: "2026-08-13T00:00:00Z".to_string(),
            favorited_at: None,
            text: text.map(str::to_string),
            html: None,
            rtf: None,
            image: image_path.map(|p| ClipboardImage {
                path: p.to_string(),
                size: 1,
                width: 1,
                height: 1,
                missing: false,
            }),
            files: None,
        }
    }

    #[test]
    fn fingerprint_text_prefers_text() {
        let a = entry("a", Some("abc"), Some("/img/1.png"));
        let b = entry("b", Some("abc"), Some("/img/2.png")); // 文本相同、图片不同 → 判重
        assert!(fingerprint_matches(&a, &b));
    }

    #[test]
    fn fingerprint_image_path_fallback() {
        let a = entry("a", None, Some("/img/1.png"));
        let b = entry("b", None, Some("/img/1.png"));
        assert!(fingerprint_matches(&a, &b));

        let c = entry("c", None, Some("/img/2.png"));
        assert!(!fingerprint_matches(&a, &c));
    }

    #[test]
    fn fingerprint_rtf_fallback() {
        let mut a = entry("a", None, None);
        a.rtf = Some("{\\rtf1 abc}".to_string());
        let mut b = entry("b", None, None);
        b.rtf = Some("{\\rtf1 abc}".to_string());
        assert!(fingerprint_matches(&a, &b));

        let mut c = entry("c", None, None);
        c.rtf = Some("{\\rtf1 xyz}".to_string());
        assert!(!fingerprint_matches(&a, &c));
    }

    #[test]
    fn fingerprint_no_common_content() {
        let a = entry("a", Some("x"), None);
        let b = entry("b", None, Some("/img/1.png"));
        assert!(!fingerprint_matches(&a, &b));
    }

    #[test]
    fn dedup_promotes_existing_and_keeps_id() {
        let mut entries = vec![entry("newer", Some("b"), None)];
        let incoming = entry("incoming", Some("a"), None);
        let outcome = dedup_promote_and_evict(&mut entries, incoming, 64);
        assert!(outcome.is_new);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "incoming");
        assert_eq!(outcome.evicted_files.len(), 0);

        // 相同文本再次捕捉 → 置顶既有条目，id 保持原值
        let outcome2 = dedup_promote_and_evict(&mut entries, entry("dup", Some("a"), None), 64);
        assert!(!outcome2.is_new);
        assert_eq!(outcome2.entry.id, "incoming");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "incoming");
    }

    #[test]
    fn dedup_updates_captured_at() {
        let mut entries = vec![];
        let outcome = dedup_promote_and_evict(
            &mut entries,
            entry("a", Some("same"), None),
            64,
        );
        assert_eq!(outcome.entry.captured_at, "2026-08-13T00:00:00Z");

        let mut later = entry("a", Some("same"), None);
        later.captured_at = "2026-08-13T01:00:00Z".to_string();
        let outcome2 = dedup_promote_and_evict(&mut entries, later, 64);
        assert_eq!(outcome2.entry.captured_at, "2026-08-13T01:00:00Z");
    }

    #[test]
    fn evict_removes_oldest_and_collects_image_paths() {
        let mut entries = vec![
            entry("newest", Some("c"), Some("/img/c.png")),
            entry("middle", Some("b"), None),
            entry("oldest", Some("a"), Some("/img/a.png")),
        ];
        let evicted = evict_over_limit(&mut entries, 2);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].id, "middle");
        assert_eq!(evicted, vec![PathBuf::from("/img/a.png")]);
    }

    #[test]
    fn evict_over_limit_repeatedly() {
        let mut entries: Vec<ClipboardEntry> = (0..5)
            .map(|i| entry(&format!("e{i}"), Some("x"), None))
            .collect();
        let evicted = evict_over_limit(&mut entries, 2);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "e0");
        assert_eq!(entries[1].id, "e1");
        assert_eq!(evicted.len(), 0);
    }

    #[test]
    fn insert_triggers_immediate_eviction() {
        // entries[0] 最新、队尾最旧；插入超限后淘汰最旧（队尾 e2）
        let mut entries: Vec<ClipboardEntry> = (0..3)
            .map(|i| entry(&format!("e{i}"), Some(&format!("t{i}")), Some(&format!("/img/{i}.png"))))
            .collect();
        let incoming = entry("new", Some("t99"), None);
        let outcome = dedup_promote_and_evict(&mut entries, incoming, 3);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "new");
        assert_eq!(entries[1].id, "e0");
        assert_eq!(entries[2].id, "e1");
        assert_eq!(outcome.evicted_files, vec![PathBuf::from("/img/2.png")]);
    }

    #[test]
    fn orphan_files_computes_difference() {
        let entries = vec![entry("a", None, Some("/img/keep.png"))];
        let dir_files = vec![
            PathBuf::from("/img/keep.png"),
            PathBuf::from("/img/orphan1.png"),
            PathBuf::from("/img/orphan2.png"),
        ];
        let orphans = orphan_files(&entries, &dir_files);
        assert_eq!(
            orphans,
            vec![PathBuf::from("/img/orphan1.png"), PathBuf::from("/img/orphan2.png")]
        );
    }

    /// 回归：条目路径与扫描目录分隔符表示不一致（Windows 上 `/` vs `\`）时不得误判孤儿。
    /// 曾导致全部图片被当作孤儿删除（scanned=2 referenced=2 removed=2）。
    #[cfg(windows)]
    #[test]
    fn orphan_files_separator_insensitive() {
        // 条目用反斜杠、扫描目录用正斜杠，同一文件
        let entries = vec![entry("a", None, Some("/img\\keep.png"))];
        let dir_files = vec![PathBuf::from("/img/keep.png")];
        assert!(orphan_files(&entries, &dir_files).is_empty());

        // 反向表示
        let entries2 = vec![entry("b", None, Some("/img/keep.png"))];
        let dir_files2 = vec![PathBuf::from("/img\\keep.png")];
        assert!(orphan_files(&entries2, &dir_files2).is_empty());
    }

    #[test]
    fn orphan_files_empty_references() {
        let dir_files = vec![PathBuf::from("/img/a.png")];
        assert_eq!(orphan_files(&[], &dir_files), dir_files);
        let empty: Vec<PathBuf> = Vec::new();
        let none = orphan_files(&[], &empty);
        assert!(none.is_empty());
    }

    #[test]
    fn empty_entry_detection() {
        assert!(entry("a", None, None).is_empty());
        let mut with_rtf = entry("b", None, None);
        with_rtf.rtf = Some("x".to_string());
        assert!(!with_rtf.is_empty());
        assert!(!entry("c", Some("x"), None).is_empty());
    }

    #[test]
    #[allow(clippy::assertions_on_constants)] // 契约固定值断言，防止将来误改上限
    fn max_entries_constants() {
        assert!(DEFAULT_MAX_ENTRIES < MAX_ENTRIES_LIMIT);
        assert_eq!(MAX_ENTRIES_LIMIT, 1024);
        assert_eq!(DEFAULT_MAX_ENTRIES, 64);
    }

    #[test]
    fn sort_favorites_first_then_by_favorited_at_desc() {
        let mut entries = vec![
            entry("plain-old", Some("p"), None),
            entry("fav-old", Some("f"), None),
            entry("fav-new", Some("f2"), None),
        ];
        entries[1].favorited_at = Some("2026-08-13T01:00:00Z".to_string());
        entries[2].favorited_at = Some("2026-08-13T02:00:00Z".to_string());
        sort_for_display(&mut entries);
        assert_eq!(entries[0].id, "fav-new"); // 收藏区在前，区内最近收藏最前
        assert_eq!(entries[1].id, "fav-old");
        assert_eq!(entries[2].id, "plain-old");
    }

    #[test]
    fn sort_keeps_recency_order_within_groups() {
        // 普通区按 capturedAt 倒序（稳定排序保持原相对序）
        let mut entries = vec![entry("older", Some("a"), None), entry("newer", Some("b"), None)];
        entries[0].captured_at = "2026-08-13T00:00:00Z".to_string();
        entries[1].captured_at = "2026-08-13T01:00:00Z".to_string();
        sort_for_display(&mut entries);
        assert_eq!(entries[0].id, "newer");
        assert_eq!(entries[1].id, "older");
    }

    #[test]
    fn evict_skips_favorites_and_removes_oldest_non_favorite() {
        let mut entries = vec![
            entry("newest", Some("c"), Some("/img/c.png")),
            entry("mid", Some("b"), None),
            entry("fav1", Some("f"), Some("/img/f.png")),
            entry("oldest", Some("a"), Some("/img/a.png")),
        ];
        entries[2].favorited_at = Some("2026-08-13T01:00:00Z".to_string());
        let evicted = evict_over_limit(&mut entries, 2);
        // 非收藏 3 > 上限 2 → 淘汰最旧非收藏（oldest）；收藏条目豁免
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|e| e.id == "fav1"));
        assert!(!entries.iter().any(|e| e.id == "oldest"));
        assert_eq!(evicted, vec![PathBuf::from("/img/a.png")]);
    }

    #[test]
    fn evict_all_favorites_evicts_nothing() {
        let mut entries = vec![
            entry("f1", Some("a"), Some("/img/a.png")),
            entry("f2", Some("b"), Some("/img/b.png")),
        ];
        entries[0].favorited_at = Some("2026-08-13T01:00:00Z".to_string());
        entries[1].favorited_at = Some("2026-08-13T02:00:00Z".to_string());
        let evicted = evict_over_limit(&mut entries, 1);
        assert!(evicted.is_empty());
        assert_eq!(entries.len(), 2); // 全部收藏 → 永不淘汰
    }

    #[test]
    fn evict_over_limit_keeps_favorites_at_tail() {
        // 收藏条目位于队尾（最旧位置）也不被淘汰
        let mut entries = vec![
            entry("new", Some("c"), None),
            entry("mid", Some("b"), None),
            entry("oldest", Some("a"), Some("/img/a.png")),
            entry("fav-tail", Some("f"), Some("/img/fav.png")),
        ];
        entries[3].favorited_at = Some("2026-08-13T00:00:00Z".to_string());
        let evicted = evict_over_limit(&mut entries, 2);
        // 非收藏 3 > 上限 2 → 淘汰最旧非收藏（oldest），队尾收藏条目豁免
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].id, "fav-tail");
        assert!(!entries.iter().any(|e| e.id == "oldest"));
        assert_eq!(evicted, vec![PathBuf::from("/img/a.png")]);
    }

    #[test]
    fn set_favorite_sets_clears_and_refreshes() {
        let mut entries = vec![entry("a", Some("x"), None), entry("b", Some("y"), None)];

        assert!(set_favorite(&mut entries, "a", true, "2026-08-13T01:00:00Z"));
        assert_eq!(entries[0].favorited_at.as_deref(), Some("2026-08-13T01:00:00Z"));
        assert!(entries[0].is_favorite());

        // 重复收藏 → 刷新收藏时间（收藏区重新置顶，幂等）
        assert!(set_favorite(&mut entries, "a", true, "2026-08-13T02:00:00Z"));
        assert_eq!(entries[0].favorited_at.as_deref(), Some("2026-08-13T02:00:00Z"));

        // 取消收藏 → 清空标志（不触发淘汰，调用方按契约容忍超限）
        assert!(set_favorite(&mut entries, "a", false, "2026-08-13T03:00:00Z"));
        assert!(!entries[0].is_favorite());
        assert!(entries[0].favorited_at.is_none());

        // 目标不存在 → false
        assert!(!set_favorite(&mut entries, "missing", true, "2026-08-13T04:00:00Z"));
    }

    #[test]
    fn dedup_promote_preserves_favorite_status() {
        let mut entries = vec![];
        let outcome = dedup_promote_and_evict(&mut entries, entry("a", Some("same"), None), 64);
        assert!(outcome.is_new);
        set_favorite(&mut entries, "a", true, "2026-08-13T01:00:00Z");

        // 相同文本再次捕捉 → 置顶既有条目，收藏状态与收藏时间均保留
        let outcome2 = dedup_promote_and_evict(&mut entries, entry("dup", Some("same"), None), 64);
        assert!(!outcome2.is_new);
        assert_eq!(entries[0].id, "a");
        assert!(entries[0].is_favorite());
        assert_eq!(
            entries[0].favorited_at.as_deref(),
            Some("2026-08-13T01:00:00Z") // 去重置顶只刷新 capturedAt，不动 favoritedAt
        );
    }

    #[test]
    fn serde_roundtrip_preserves_favorite_and_defaults_for_old_data() {
        // 收藏条目序列化 → 反序列化后状态保持
        let mut fav = entry("a", Some("x"), None);
        fav.favorited_at = Some("2026-08-13T01:00:00Z".to_string());
        let json = serde_json::to_string(&fav).unwrap();
        let back: ClipboardEntry = serde_json::from_str(&json).unwrap();
        assert!(back.is_favorite());

        // 旧数据（无 favoritedAt 字段）反序列化 → 未收藏（serde default，零迁移）
        let old_json = r#"{"id":"a","capturedAt":"2026-08-13T00:00:00Z","text":"x"}"#;
        let old: ClipboardEntry = serde_json::from_str(old_json).unwrap();
        assert!(!old.is_favorite());
        assert!(old.favorited_at.is_none());
    }
}
