//! lan_file 模块根（0.3.0，契约 `docs/api/lan-file.md`）。
//!
//! - `commands.rs`：7 命令薄壳 + 4 事件
//! - `service.rs`：纯逻辑（类型/状态机/公告 peers/校验/EMA）
//! - `state.rs`：运行时共享态 + 监听器 + 图片通道（Tauri 胶水）
//! - `store.rs`：lan-file.json / sidecar / 落盘工具
//! - `transport/`：VLF/1 协议（crypto + proto，纯逻辑）与 TCP 会话层
//! - `tests.rs`：开发者测试（dt）
//!
//! 阶段性构建注记：types/store/service 先行落地（dt 全绿），
//! commands/state 在后续实现节接入，未消费的类型暂以 `#[allow(dead_code)]` 抑制。

pub mod commands;
pub mod service;
pub mod state;
pub mod store;
pub mod transport;

#[cfg(test)]
mod tests;

pub use commands::*;
