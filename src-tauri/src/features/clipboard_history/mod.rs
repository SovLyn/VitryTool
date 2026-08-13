//! 剪贴板历史功能域（clipboard-history）。
//!
//! - `commands.rs`：`#[tauri::command]` 薄壳
//! - `service.rs`：纯逻辑业务核心（去重置顶 / 即时淘汰 / 孤儿计算）
//! - `store.rs`：持久化抽象 + tauri-plugin-store 实现
//! - `tests.rs`：开发者测试（dt）
//!
//! 契约：`docs/api/clipboard-history.md`；领域术语：`dev/CONTEXT.md`。

pub mod commands;
pub mod service;
mod store;

#[cfg(test)]
mod tests;

pub use commands::*;
