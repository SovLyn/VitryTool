//! VitryTool 后端库入口。
//!
//! 本文件只做三件事：声明模块、组装 Tauri builder（插件 / 状态 / 事件）、注册命令。
//! 业务逻辑一律放在 `core/`（横切基础）与 `features/`（功能域）中，
//! 见 `docs/architecture.md`。

/// 横切基础模块（`error`、`state`）为 `pub`，供 doctest 与外部消费方使用。
pub mod core;
mod features;

use core::state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
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
        // 单实例（lan-sync 前提：一台机器一个终端，见契约 docs/api/lan-sync.md 5.1）；
        // 第二实例启动时唤出主窗口
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .manage(AppState::default())
        .setup(|app| {
            core::tray::init(app)?;
            features::quick_paste::init(app)?;
            features::lan_sync::init_node(app)?;
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
            features::clipboard_history::set_entry_favorite,
            features::clipboard_history::clear_clipboard_history,
            features::clipboard_history::cleanup_orphan_images,
            features::clipboard_history::get_max_entries,
            features::clipboard_history::set_max_entries,
            // 快速粘贴（features/quick_paste）
            features::quick_paste::get_hotkey,
            features::quick_paste::set_hotkey,
            features::quick_paste::get_hotkey_capability,
            features::quick_paste::quick_paste_ready,
            features::quick_paste::quick_paste_close,
            // 托盘（core/tray，契约 quick-paste 5.5：菜单文案 i18n）
            core::tray::set_tray_labels,
            // 局域网同步（features/lan_sync）
            features::lan_sync::get_lan_sync_status,
            features::lan_sync::set_lan_sync_broadcast,
            features::lan_sync::set_lan_sync_receive,
            features::lan_sync::set_lan_sync_terminal_name,
            features::lan_sync::get_lan_inbox,
            features::lan_sync::write_lan_inbox_entry,
            features::lan_sync::delete_lan_inbox_entry,
            features::lan_sync::clear_lan_inbox,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // 退出清理：停止 libp2p 节点线程（RunEvent::Exit）
    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(mut node) = app_handle
                .state::<AppState>()
                .peer_node
                .lock()
                .unwrap()
                .take()
            {
                node.shutdown();
            }
        }
    });
}
