//! 开发者测试（dt）：持久化抽象的内存实现 + 命令核心流程的行为验证。
//!
//! 纯逻辑（去重/淘汰/孤儿）的单元测试在 `service.rs` 内；这里验证
//! 「命令级流程」与 store 抽象的组合，不依赖 Tauri 运行时。

use super::service::{
    dedup_promote_and_evict, evict_over_limit, set_favorite, sort_for_display, ClipboardEntry,
    ClipboardImage,
};
use super::store::HistoryStore;
use crate::core::error::ApiError;
use std::sync::Mutex;

/// 内存版持久化（替代 StoreBackend 用于测试）。
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

struct Inner {
    entries: Vec<ClipboardEntry>,
    max_entries: usize,
}

impl MemoryStore {
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: Vec::new(),
                max_entries,
            }),
        }
    }
}

impl HistoryStore for MemoryStore {
    fn load_entries(&self) -> Result<Vec<ClipboardEntry>, ApiError> {
        Ok(self.inner.lock().unwrap().entries.clone())
    }

    fn save_entries(&self, entries: &[ClipboardEntry]) -> Result<(), ApiError> {
        self.inner.lock().unwrap().entries = entries.to_vec();
        Ok(())
    }

    fn load_max_entries(&self) -> Result<usize, ApiError> {
        Ok(self.inner.lock().unwrap().max_entries)
    }

    fn save_max_entries(&self, n: usize) -> Result<(), ApiError> {
        self.inner.lock().unwrap().max_entries = n;
        Ok(())
    }
}

fn entry(id: &str, text: &str, image: Option<&str>) -> ClipboardEntry {
    ClipboardEntry {
        id: id.to_string(),
        captured_at: "2026-08-13T00:00:00Z".to_string(),
        favorited_at: None,
        text: Some(text.to_string()),
        html: None,
        rtf: None,
        image: image.map(|p| ClipboardImage {
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
fn capture_flow_persists_and_evicts() {
    let store = MemoryStore::new(2);
    let mut entries = store.load_entries().unwrap();

    // 三次新增，上限 2 → 淘汰最旧
    for (i, text) in ["first", "second", "third"].iter().enumerate() {
        let incoming = entry(&format!("id-{i}"), text, None);
        let outcome =
            dedup_promote_and_evict(&mut entries, incoming, store.load_max_entries().unwrap());
        assert!(outcome.is_new);
        store.save_entries(&entries).unwrap();
    }

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id, "id-2");
    assert_eq!(entries[1].id, "id-1");
    assert_eq!(store.load_entries().unwrap().len(), 2);
}

#[test]
fn dedup_does_not_evict() {
    let store = MemoryStore::new(1);
    let mut entries = store.load_entries().unwrap();
    let first = dedup_promote_and_evict(&mut entries, entry("a", "same", None), 1);
    assert!(first.is_new);

    let second = dedup_promote_and_evict(&mut entries, entry("b", "same", None), 1);
    assert!(!second.is_new);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "a");
}

#[test]
fn max_entries_persisted_and_applied() {
    let store = MemoryStore::new(64);
    assert_eq!(store.load_max_entries().unwrap(), 64);
    store.save_max_entries(3).unwrap();
    assert_eq!(store.load_max_entries().unwrap(), 3);

    // 模拟 setMaxEntries 截断：现有多余条目被淘汰并返回图片路径
    let mut entries = vec![
        entry("newest", "c", Some("/img/c.png")),
        entry("middle", "b", Some("/img/b.png")),
        entry("oldest", "a", Some("/img/a.png")),
    ];
    let evicted = evict_over_limit(&mut entries, 2);
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].to_string_lossy(), "/img/a.png");
}

/// 收藏流程组合（契约 5.8）：收藏豁免淘汰、展示排序、取消收藏容忍超限、去重置顶保持收藏。
#[test]
fn favorite_flow_survives_eviction_and_sorts_to_top() {
    let store = MemoryStore::new(2);
    let mut entries = store.load_entries().unwrap();

    // 捕捉三条（上限 2）→ 淘汰最旧的普通条目
    for (i, text) in ["a", "b", "c"].iter().enumerate() {
        let outcome =
            dedup_promote_and_evict(&mut entries, entry(&format!("id-{i}"), text, None), 2);
        assert!(outcome.is_new);
    }
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id, "id-2");
    assert_eq!(entries[1].id, "id-1");

    // 收藏最旧的 id-1（此刻位于队尾）
    assert!(set_favorite(
        &mut entries,
        "id-1",
        true,
        "2026-08-13T01:00:00Z"
    ));
    store.save_entries(&entries).unwrap();

    // 继续捕捉两条 → 淘汰最旧的非收藏（id-2），收藏的 id-1 豁免
    for (i, text) in ["d", "e"].iter().enumerate() {
        let outcome =
            dedup_promote_and_evict(&mut entries, entry(&format!("id-{}", 3 + i), text, None), 2);
        assert!(outcome.is_new);
    }
    assert_eq!(entries.len(), 3); // 2 非收藏 + 1 收藏（收藏豁免上限）
    assert!(entries.iter().any(|e| e.id == "id-1" && e.is_favorite()));
    assert!(!entries.iter().any(|e| e.id == "id-2")); // 最旧非收藏被淘汰

    // 展示序：收藏区在前
    sort_for_display(&mut entries);
    assert_eq!(entries[0].id, "id-1");
    assert!(entries.iter().skip(1).all(|e| !e.is_favorite()));

    // 取消收藏 → 不触发淘汰（容忍短暂超限：普通条目数 3 > 上限 2 仍全部保留）
    assert!(set_favorite(
        &mut entries,
        "id-1",
        false,
        "2026-08-13T02:00:00Z"
    ));
    store.save_entries(&entries).unwrap();
    assert_eq!(entries.len(), 3);
    assert!(!entries.iter().any(|e| e.is_favorite()));
}
