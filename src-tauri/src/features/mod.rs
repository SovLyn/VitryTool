//! 功能域模块：每个功能一个独立 mod。
//!
//! 新功能的标准流程（详见 `docs/architecture.md` 与 `docs/api/README.md`）：
//!
//! 1. **接口契约先行**：在 `docs/api/<feature>.md` 规划接口（用模板 `docs/api/TEMPLATE.md`）。
//! 2. 在 `src-tauri/src/features/` 新建 `<feature>/` 目录，包含：
//!    - `mod.rs` —— 模块结构声明与公开导出
//!    - `commands.rs` —— `#[tauri::command]` 薄壳（只做参数解析与状态获取）
//!    - `service.rs` —— 业务逻辑（脱离 Tauri 上下文可独立测试）
//!    - `tests.rs` —— 开发者测试（dt）：单元测试 / doctest（**必须**，覆盖度不做硬性要求）
//! 3. 在 `lib.rs` 的 `invoke_handler` 注册命令（按功能分组注释）。
//! 4. 编写功能文档 `docs/features/<feature>.md`，并按 `docs/versioning.md` 递增版本。

/// 剪贴板历史（首个功能）。
pub mod clipboard_history;

/// 快速粘贴（全局快捷键 + 置顶小屏）。
pub mod quick_paste;

/// 局域网剪贴板同步（0.2.5；节点层见 core/peer_node）。
pub mod lan_sync;
