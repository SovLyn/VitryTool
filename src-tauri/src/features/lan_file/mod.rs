//! lan_file 模块根（0.3.0，契约 `docs/api/lan-file.md`）。
//!
//! - `commands.rs`：8 命令薄壳 + 4 事件（仅桌面注册，契约 5.9）
//! - `service.rs`：纯逻辑（类型/状态机/公告 peers/校验/EMA/路径预检）
//! - `state.rs`：运行时共享态 + 监听器 + 图片通道（Tauri 胶水）
//! - `store.rs`：lan-file.json / sidecar / 落盘工具
//! - `transport/`：VLF/1 协议（crypto + proto，纯逻辑）与 TCP 会话层
//! - `tests.rs`：开发者测试（dt）

pub mod commands;
pub mod service;
pub mod state;
pub mod store;
pub mod transport;

#[cfg(test)]
mod tests;

/// 命令薄壳再导出（`lib.rs` 桌面 handler 用）；移动端不注册任何 lan-file 命令。
#[cfg(desktop)]
pub use commands::*;
