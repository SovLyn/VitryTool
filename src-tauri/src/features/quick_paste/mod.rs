//! 快速粘贴功能域（quick-paste）。
//!
//! 全局快捷键 + 置顶小屏：按住快捷键唤出剪贴板历史列表，滚轮选择，松开回写。
//! - `commands.rs`：`#[tauri::command]` 薄壳 + 快捷键事件处理 + popup 窗口管理
//! - `service.rs`：纯逻辑业务核心（快捷键解析 / 规范化 / 校验）
//! - `store.rs`：快捷键设置持久化（tauri-plugin-store 实现）
//! - `tests.rs`：开发者测试（dt）
//!
//! 契约：`docs/api/quick-paste.md`；领域术语沿用 `dev/CONTEXT.md`。

pub mod commands;
pub mod service;
mod store;

#[cfg(test)]
mod tests;

pub use commands::*;
