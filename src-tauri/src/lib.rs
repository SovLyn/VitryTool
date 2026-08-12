//! VitryTool 后端库入口。
//!
//! 本文件只做两件事：声明模块、组装 Tauri builder。
//! 业务逻辑一律放在 `core/`（横切基础）与 `features/`（功能域）中，
//! 见 `docs/architecture.md`。

/// 横切基础模块（`error`、`state`）为 `pub`，供 doctest 与外部消费方使用。
pub mod core;
mod features;

use core::state::AppState;

/// 脚手架示例命令（来自 create-tauri-app 模板）。
///
/// 仅用于验证前后端最小链路，**首个功能落地后移除**，
/// 相关前端演示代码（`src/App.tsx` 中的 greet 调用）同步清理。
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
