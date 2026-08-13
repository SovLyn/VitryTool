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

/// 超过上限时删除最旧条目（队尾），返回被删条目的图片文件路径（调用方负责删除）。
pub fn evict_over_limit(entries: &mut Vec<ClipboardEntry>, max_entries: usize) -> Vec<PathBuf> {
    let mut evicted = Vec::new();
    while entries.len() > max_entries {
        if let Some(oldest) = entries.pop() {
            if let Some(img) = oldest.image {
                evicted.push(PathBuf::from(img.path));
            }
        }
    }
    evicted
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
}
