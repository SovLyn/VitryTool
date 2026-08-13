//! 横切基础模块：错误、全局状态、通用工具。
//!
//! 功能间不互相依赖，需要复用的能力收敛到这里。

pub mod error;
pub mod log;
pub mod state;
pub mod tray;
