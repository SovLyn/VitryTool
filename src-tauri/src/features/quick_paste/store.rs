//! 快捷键设置持久化：tauri-plugin-store 实现（store 文件 `quick-paste.json`，键 `hotkey`）。
//!
//! service 层不直接依赖 Tauri；命令通过本模块读写 store。

use crate::core::error::ApiError;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_store::{Store, StoreExt};

/// 统一存储错误码（契约第 4 节）。
const ERR_STORAGE: &str = "quick_paste.storage_error";

fn storage_err(err: impl std::fmt::Display) -> ApiError {
    log::error!("{ERR_STORAGE}: {err}");
    ApiError::new(ERR_STORAGE, format!("quick-paste settings store error: {err}"))
}

/// 快捷键设置持久化。store 文件位于应用数据目录下的 `quick-paste.json`。
pub struct HotkeyStore {
    store: Arc<Store<Wry>>,
}

impl HotkeyStore {
    pub fn new(app: &AppHandle) -> Result<Self, ApiError> {
        let data_dir = app.path().app_data_dir().map_err(storage_err)?;
        let store = app
            .store(data_dir.join("quick-paste.json"))
            .map_err(storage_err)?;
        Ok(Self { store })
    }

    /// 读取当前快捷键；未设置返回 `None`。
    pub fn load_hotkey(&self) -> Result<Option<String>, ApiError> {
        match self.store.get("hotkey") {
            Some(value) => serde_json::from_value(value).map_err(storage_err),
            None => Ok(None),
        }
    }

    /// 保存快捷键（标准格式字符串）。
    pub fn save_hotkey(&self, hotkey: &str) -> Result<(), ApiError> {
        let value = serde_json::to_value(hotkey).map_err(storage_err)?;
        self.store.set("hotkey", value);
        self.store.save().map_err(storage_err)
    }

    /// 清除快捷键设置。
    pub fn clear_hotkey(&self) -> Result<(), ApiError> {
        self.store.delete("hotkey");
        self.store.save().map_err(storage_err)
    }
}
