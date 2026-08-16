//! lan-sync 开发者测试（dt）。
//!
//! 纯逻辑测试集中在 `service.rs` 的 `pure_tests` 模块；
//! 此处补 store 抽象与状态层不依赖 Tauri 的用例。

use super::service::{
    insert_message, InboxData, InboxEntry, LanSettings, MAX_ENTRIES_PER_NODE, MAX_NODES,
};
use super::store::{InboxStore, MemoryStore, SettingsStore};
use crate::core::error::ApiError;

fn sample_entry(peer: &str, fp: &str, recv: &str) -> InboxEntry {
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
fn memory_store_implements_traits() {
    let store = MemoryStore::default();
    assert_eq!(store.load_inbox().unwrap(), InboxData::default());
    assert_eq!(store.load_settings().unwrap(), LanSettings::default());
    let _: Result<(), ApiError> = store.save_inbox(&InboxData::default());
    let _: Result<(), ApiError> = store.save_settings(&LanSettings::default());
}

#[test]
fn settings_defaults_all_on() {
    let s = LanSettings::default();
    assert!(s.broadcast_enabled);
    assert!(s.receive_enabled);
}

#[test]
fn inbox_capacity_constants_match_contract() {
    assert_eq!(MAX_ENTRIES_PER_NODE, 8);
    assert_eq!(MAX_NODES, 8);
}

#[test]
fn inbox_persists_shape_roundtrip() {
    // 验证 InboxData 的 serde 形状（camelCase）与持久化文件约定一致
    let mut data = InboxData::default();
    insert_message(&mut data, sample_entry("p1", "f1", "2026-08-14T10:00:00Z"));
    let json = serde_json::to_value(&data).unwrap();
    assert!(json.get("nodes").is_some());
    let node = &json["nodes"][0];
    assert!(node.get("peerId").is_some());
    assert!(node.get("terminalName").is_some());
    let entry = &node["entries"][0];
    assert!(entry.get("receivedAt").is_some());
    assert!(entry.get("sentAt").is_some());
    assert!(entry.get("fingerprint").is_some());
    // 回读
    let back: InboxData = serde_json::from_value(json).unwrap();
    assert_eq!(back, data);
}
