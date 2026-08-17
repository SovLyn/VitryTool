//! VitryTool 后端库入口。
//!
//! 本文件只做三件事：声明模块、组装 Tauri builder（插件 / 状态 / 事件）、注册命令。
//! 业务逻辑一律放在 `core/`（横切基础）与 `features/`（功能域）中，
//! 见 `docs/architecture.md`。
//!
//! 平台差异（0.2.9，契约 `docs/api/mobile.md`）：
//! - 桌面专属插件 / 功能（clipboard-x、global-shortcut、window-state、
//!   single-instance、托盘、quick_paste）以 `#[cfg(desktop)]` 门控；
//! - 移动端注册 clipboard-manager（写剪贴板）并运行 lan-sync 节点（接收）；
//! - 命令注册分平台组合（`generate_handler!` 为 proc macro，参数内不能写 cfg，
//!   故拆「公共命令」+「桌面专属命令」两个 handler 由闭包组合）。

/// 横切基础模块（`error`、`state`、`platform`）为 `pub`，供 doctest 与外部消费方使用。
pub mod core;
mod features;

use core::state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(core::log::plugin())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build());

    // ---- 桌面专属插件（移动端不编译，契约 mobile 6） ----
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_clipboard_x::init())
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
            }));
    }

    // ---- 移动端专属插件 ----
    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_clipboard_manager::init());
    }

    // 移动端无 on_window_event 分支，mut 仅在桌面需要（cfg_attr 抑制移动端 unused_mut）
    #[cfg_attr(not(desktop), allow(unused_mut))]
    let mut builder = builder.manage(AppState::default()).setup(|app| {
        // 桌面：托盘 + 快速粘贴（全局快捷键）
        #[cfg(desktop)]
        {
            core::tray::init(app)?;
            features::quick_paste::init(app)?;
        }
        // 移动端：注册「写剪贴板 + 显式入历史」钩子（lan-sync 回写经 core::hooks 调用）
        #[cfg(mobile)]
        {
            features::clipboard_history::register_mobile_clipboard_write();
        }
        // 两平台：lan-sync 节点（移动端前台运行，契约 mobile 5.4）
        features::lan_sync::init_node(app)?;
        Ok(())
    });

    // 托盘常驻：任何窗口的「关闭」都改为隐藏（契约 5.5；桌面专属）
    #[cfg(desktop)]
    {
        builder = builder.on_window_event(features::quick_paste::on_window_event);
    }

    builder
        .invoke_handler(build_invoke_handler())
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                // 先置位关闭标记，消费者线程据此不误报「节点运行时错误」通知
                // （正常退出 vs 节点崩溃的区分，见 docs/api/notify.md 5.2）
                features::lan_sync::state::mark_shutting_down();
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

/// 命令注册：按平台生成命令列表（`generate_handler!` 为 proc macro，参数内不能写 cfg，
/// 故分平台两套完整列表；公共命令见各列表上半部分，桌面专属命令见 `#[cfg(desktop)]` 分支）。
#[cfg(desktop)]
fn build_invoke_handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static
{
    tauri::generate_handler![
        // 平台识别（0.2.9，契约 mobile）
        core::platform::get_platform_info,
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
        // 通知（core/notify，契约 docs/api/notify.md）
        core::notify::notify,
    ]
}

/// 命令注册（移动端）：无快速粘贴 / 托盘 / 捕捉命令（前端无入口，契约 mobile 2）。
#[cfg(mobile)]
fn build_invoke_handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static
{
    tauri::generate_handler![
        // 平台识别（0.2.9，契约 mobile）
        core::platform::get_platform_info,
        // 剪贴板历史（features/clipboard_history）
        features::clipboard_history::get_clipboard_history,
        features::clipboard_history::write_clipboard_entry,
        features::clipboard_history::delete_clipboard_entry,
        features::clipboard_history::set_entry_favorite,
        features::clipboard_history::clear_clipboard_history,
        features::clipboard_history::get_max_entries,
        features::clipboard_history::set_max_entries,
        // 局域网同步（features/lan_sync）
        features::lan_sync::get_lan_sync_status,
        features::lan_sync::set_lan_sync_broadcast,
        features::lan_sync::set_lan_sync_receive,
        features::lan_sync::set_lan_sync_terminal_name,
        features::lan_sync::get_lan_inbox,
        features::lan_sync::write_lan_inbox_entry,
        features::lan_sync::delete_lan_inbox_entry,
        features::lan_sync::clear_lan_inbox,
        // 通知（core/notify，契约 docs/api/notify.md）
        core::notify::notify,
    ]
}
