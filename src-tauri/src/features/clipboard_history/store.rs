//! 历史数据持久化：抽象接口 + tauri-plugin-store 实现。
//!
//! service 层不直接依赖 Tauri；命令通过本模块读写 store（单文件双键：`history` / `maxEntries`，
//! 见 `docs/api/clipboard-history.md` 第 5.3 节）。

use super::service::{ClipboardEntry, DEFAULT_MAX_ENTRIES};
use crate::core::error::ApiError;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_store::{Store, StoreExt};

/// 统一存储错误码（契约第 4 节）。
const ERR_STORAGE: &str = "clipboard.storage_error";

fn storage_err(err: impl std::fmt::Display) -> ApiError {
    log::error!("{ERR_STORAGE}: {err}");
    ApiError::new(ERR_STORAGE, format!("history store error: {err}"))
}

/// 持久化抽象：便于在单元测试中用内存实现替代（docs/architecture.md 第 3 节「命令薄壳、业务可测」）。
pub trait HistoryStore: Send + Sync {
    fn load_entries(&self) -> Result<Vec<ClipboardEntry>, ApiError>;
    fn save_entries(&self, entries: &[ClipboardEntry]) -> Result<(), ApiError>;
    /// 当前上限；未设置时返回默认值（DEFAULT_MAX_ENTRIES）。
    fn load_max_entries(&self) -> Result<usize, ApiError>;
    fn save_max_entries(&self, n: usize) -> Result<(), ApiError>;
}

/// 基于 tauri-plugin-store 的实现：store 文件位于应用数据目录下的 `clipboard.json`。
///
/// `app.store(path)` 对同一路径返回缓存的同一实例（store.rs 按 path 复用），
/// 因此每次命令获取到的都是共享内存态，`set` 自动触发持久化。
pub struct StoreBackend {
    store: Arc<Store<Wry>>,
}

impl StoreBackend {
    pub fn new(app: &AppHandle) -> Result<Self, ApiError> {
        let data_dir = app.path().app_data_dir().map_err(storage_err)?;
        let store = app.store(data_dir.join("clipboard.json")).map_err(storage_err)?;
        Ok(Self { store })
    }
}

impl HistoryStore for StoreBackend {
    fn load_entries(&self) -> Result<Vec<ClipboardEntry>, ApiError> {
        match self.store.get("history") {
            Some(value) => serde_json::from_value(value).map_err(storage_err),
            None => Ok(Vec::new()),
        }
    }

    fn save_entries(&self, entries: &[ClipboardEntry]) -> Result<(), ApiError> {
        let value = serde_json::to_value(entries).map_err(storage_err)?;
        self.store.set("history", value);
        self.store.save().map_err(storage_err)
    }

    fn load_max_entries(&self) -> Result<usize, ApiError> {
        match self.store.get("maxEntries") {
            Some(value) => serde_json::from_value(value).map_err(storage_err),
            None => Ok(DEFAULT_MAX_ENTRIES),
        }
    }

    fn save_max_entries(&self, n: usize) -> Result<(), ApiError> {
        let value = serde_json::to_value(n).map_err(storage_err)?;
        self.store.set("maxEntries", value);
        self.store.save().map_err(storage_err)
    }
}
