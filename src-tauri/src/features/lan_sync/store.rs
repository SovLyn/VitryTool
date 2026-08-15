//! lan-sync 持久化：收件箱（lan-inbox.json）+ 设置（lan-sync.json），
//! 模式对齐剪贴板历史（HistoryStore：抽象 trait + tauri-plugin-store 实现 + 内存实现）。

use super::service::{InboxData, LanSettings};
use crate::core::error::ApiError;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_store::{Store, StoreExt};

const ERR_STORAGE: &str = "lan.storage_error";

fn storage_err(err: impl std::fmt::Display) -> ApiError {
    log::error!("{ERR_STORAGE}: {err}");
    ApiError::new(ERR_STORAGE, format!("lan-sync store error: {err}"))
}

/// 收件箱持久化抽象。
pub trait InboxStore: Send + Sync {
    fn load_inbox(&self) -> Result<InboxData, ApiError>;
    fn save_inbox(&self, data: &InboxData) -> Result<(), ApiError>;
}

/// 设置持久化抽象。
pub trait SettingsStore: Send + Sync {
    fn load_settings(&self) -> Result<LanSettings, ApiError>;
    fn save_settings(&self, settings: &LanSettings) -> Result<(), ApiError>;
}

/// 基于 tauri-plugin-store 的实现（收件箱与设置各一个 store 文件）。
pub struct StoreBackend {
    inbox: Arc<Store<Wry>>,
    settings: Arc<Store<Wry>>,
}

impl StoreBackend {
    pub fn new(app: &AppHandle) -> Result<Self, ApiError> {
        let data_dir = app.path().app_data_dir().map_err(storage_err)?;
        Ok(Self {
            inbox: app.store(data_dir.join("lan-inbox.json")).map_err(storage_err)?,
            settings: app.store(data_dir.join("lan-sync.json")).map_err(storage_err)?,
        })
    }
}

impl InboxStore for StoreBackend {
    fn load_inbox(&self) -> Result<InboxData, ApiError> {
        match self.inbox.get("inbox") {
            Some(value) => serde_json::from_value(value).map_err(storage_err),
            None => Ok(InboxData::default()),
        }
    }

    fn save_inbox(&self, data: &InboxData) -> Result<(), ApiError> {
        let value = serde_json::to_value(data).map_err(storage_err)?;
        self.inbox.set("inbox", value);
        self.inbox.save().map_err(storage_err)
    }
}

impl SettingsStore for StoreBackend {
    fn load_settings(&self) -> Result<LanSettings, ApiError> {
        match self.settings.get("settings") {
            Some(value) => serde_json::from_value(value).map_err(storage_err),
            None => Ok(LanSettings::default()),
        }
    }

    fn save_settings(&self, settings: &LanSettings) -> Result<(), ApiError> {
        let value = serde_json::to_value(settings).map_err(storage_err)?;
        self.settings.set("settings", value);
        self.settings.save().map_err(storage_err)
    }
}

/// 内存实现（dt 用；非测试构建不实例化）。
#[derive(Default)]
#[allow(dead_code)]
pub struct MemoryStore {
    pub inbox: InboxData,
    pub settings: LanSettings,
}

impl InboxStore for MemoryStore {
    fn load_inbox(&self) -> Result<InboxData, ApiError> {
        Ok(self.inbox.clone())
    }
    fn save_inbox(&self, data: &InboxData) -> Result<(), ApiError> {
        // 引用语义下直接改内存（测试简单场景）
        // 注：此处仅作 trait 契约测试用，真实逻辑在 state/commands 中持锁操作
        let _ = data;
        Ok(())
    }
}

impl SettingsStore for MemoryStore {
    fn load_settings(&self) -> Result<LanSettings, ApiError> {
        Ok(self.settings.clone())
    }
    fn save_settings(&self, settings: &LanSettings) -> Result<(), ApiError> {
        let _ = settings;
        Ok(())
    }
}
