//! 系统托盘（应用壳能力，横切基础）。
//!
//! 职责（契约 `docs/api/quick-paste.md` 第 5.5、5.6 节）：
//! - 创建托盘图标与菜单（「显示主窗口」「退出」）；
//! - 左键单击 / 双击唤出主窗口；
//! - 「退出」前显式保存窗口状态（window-state 插件在 close 流程保存，
//!   但 `app.exit` 直接退出不触发 close 事件，故手动落盘）。
//!
//! 注：托盘菜单文案暂为后端硬编码中文（契约 5.5 未决问题），后续如需 i18n 再评估。

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager,
};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";

/// 初始化托盘（setup 阶段调用）。
pub fn init(app: &mut App) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, MENU_SHOW, "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().expect("default window icon missing").clone())
        .tooltip("VitryTool")
        .menu(&menu)
        .show_menu_on_left_click(false) // 左键单击直接唤出窗口（右键弹菜单）
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => show_main_window(app),
            MENU_QUIT => {
                log::info!("tray: quit requested");
                let _ = app.save_window_state(StateFlags::all());
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// 唤出主窗口：显示 + 还原最小化 + 聚焦。
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        log::debug!("tray: main window shown");
    }
}
