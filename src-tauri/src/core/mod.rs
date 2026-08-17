//! 横切基础模块：错误、全局状态、通用工具、跨功能钩子、节点层。
//!
//! 功能间不互相依赖，需要复用的能力收敛到这里。

pub mod error;
pub mod hooks;
pub mod log;
pub mod notify;
pub mod peer_node;
pub mod platform;
pub mod state;
/// 系统托盘（桌面专属：依赖 tauri tray-icon feature，移动端不编译，契约 mobile 5.1）。
#[cfg(desktop)]
pub mod tray;
