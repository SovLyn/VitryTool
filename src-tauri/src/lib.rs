//! VitryTool 后端库入口。
//!
//! 本文件只做两件事：声明模块、组装 Tauri builder。
//! 业务逻辑一律放在 `core/`（横切基础）与 `features/`（功能域）中，
//! 见 `docs/architecture.md`。

/// 横切基础模块（`error`、`state`）为 `pub`，供 doctest 与外部消费方使用。
pub mod core;
mod features;

use core::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(core::log::plugin())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_x::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            // 剪贴板历史（features/clipboard_history）
            features::clipboard_history::capture_clipboard,
            features::clipboard_history::get_clipboard_history,
            features::clipboard_history::write_clipboard_entry,
            features::clipboard_history::delete_clipboard_entry,
            features::clipboard_history::clear_clipboard_history,
            features::clipboard_history::cleanup_orphan_images,
            features::clipboard_history::get_max_entries,
            features::clipboard_history::set_max_entries,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
