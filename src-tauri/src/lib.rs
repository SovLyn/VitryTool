//! VitryTool 后端库入口。
//!
//! 本文件只做三件事：声明模块、组装 Tauri builder（插件 / 状态 / 事件）、注册命令。
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
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // 窗口状态记忆：主窗口位置/大小/最大化；quick-paste 小屏每次跟随鼠标，不记忆（契约 5.6）
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_denylist(&[features::quick_paste::POPUP_LABEL])
                .build(),
        )
        .manage(AppState::default())
        .setup(|app| {
            core::tray::init(app)?;
            features::quick_paste::init(app)?;
            Ok(())
        })
        // 托盘常驻：任何窗口的「关闭」都改为隐藏（契约 5.5）
        .on_window_event(features::quick_paste::on_window_event)
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
            // 快速粘贴（features/quick_paste）
            features::quick_paste::get_hotkey,
            features::quick_paste::set_hotkey,
            features::quick_paste::quick_paste_ready,
            features::quick_paste::quick_paste_close,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
