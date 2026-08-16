//! 系统托盘（应用壳能力，横切基础）。
//!
//! 职责（契约 `docs/api/quick-paste.md` 第 5.5、5.6 节）：
//! - 创建托盘图标与菜单：主窗口操作 + lan-sync 快速开关；
//! - 左键单击 / 双击唤出主窗口；
//! - 「退出」前显式保存窗口状态（window-state 插件在 close 流程保存，
//!   但 `app.exit` 直接退出不触发 close 事件，故手动落盘）。
//!
//! 托盘菜单文案 i18n（0.2.6，契约 5.5）：文案由**前端 i18n 提供**，
//! 主窗口加载后及语言切换时调用 `set_tray_labels` 下发；后端不持有界面文案
//! （符合「后端不输出界面文案」铁律）。托盘初始化时仍以默认中文文案创建，
//! 主窗口首次下发后生效。
//!
//! lan-sync 快速开关（0.2.7）：菜单含「剪贴板广播」「剪贴板接收」两个可勾选项，
//! 经 `core::hooks` 的开关钩子读写（lan-sync 注册实现），与设置页同一持久化路径。

use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager, Wry,
};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

use crate::core::error::ApiError;
use crate::core::hooks;

const TRAY_ID: &str = "main-tray";
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";
/// lan-sync 快速开关菜单项 id。
const MENU_BROADCAST: &str = "lan-broadcast";
const MENU_RECEIVE: &str = "lan-receive";

/// 错误码（契约第 4 节）：托盘菜单文案更新失败。
const ERR_TRAY_UPDATE_FAILED: &str = "quick_paste.tray_update_failed";

/// 托盘菜单项句柄（供开关勾选状态同步；主菜单文案重建时整体替换，无需保留旧句柄）。
struct TrayItems {
    broadcast: CheckMenuItem<Wry>,
    receive: CheckMenuItem<Wry>,
}

static TRAY_ITEMS: std::sync::OnceLock<std::sync::Mutex<Option<TrayItems>>> =
    std::sync::OnceLock::new();

fn tray_items_slot() -> &'static std::sync::Mutex<Option<TrayItems>> {
    TRAY_ITEMS.get_or_init(|| std::sync::Mutex::new(None))
}

/// 初始化托盘（setup 阶段调用）。
pub fn init(app: &mut App) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, MENU_SHOW, "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;

    // lan-sync 快速开关（勾选态初始取当前设置；未初始化时默认勾选「开」）
    let broadcast_item = CheckMenuItem::with_id(
        app,
        MENU_BROADCAST,
        "剪贴板广播",
        true,
        hooks::lan_sync_broadcast_enabled().unwrap_or(true),
        None::<&str>,
    )?;
    let receive_item = CheckMenuItem::with_id(
        app,
        MENU_RECEIVE,
        "剪贴板接收",
        true,
        hooks::lan_sync_receive_enabled().unwrap_or(true),
        None::<&str>,
    )?;

    let menu = Menu::with_items(
        app,
        &[&show_item, &quit_item, &broadcast_item, &receive_item],
    )?;

    tray_items_slot().lock().unwrap().replace(TrayItems {
        broadcast: broadcast_item,
        receive: receive_item,
    });

    let _tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(app.default_window_icon().expect("default window icon missing").clone())
        .tooltip("VitryTool")
        .menu(&menu)
        .show_menu_on_left_click(false) // 左键单击直接唤出窗口（右键弹菜单）
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => show_main_window(app),
            MENU_BROADCAST => toggle_broadcast(app),
            MENU_RECEIVE => toggle_receive(app),
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

/// 更新托盘菜单文案（契约 5.5，0.2.6；快速开关文案 0.2.7）。
///
/// 用前端 i18n 下发的文案重建菜单；菜单项 id 保持不变（`MENU_SHOW` / `MENU_QUIT` /
/// `MENU_BROADCAST` / `MENU_RECEIVE`），事件路由不受影响。空文案拒绝更新
/// （返回 `quick_paste.tray_update_failed`）。
#[tauri::command]
pub fn set_tray_labels(
    app: AppHandle,
    show_main: String,
    quit: String,
    broadcast: String,
    receive: String,
) -> Result<(), ApiError> {
    if !labels_are_valid(&show_main, &quit) || !labels_are_valid(&broadcast, &receive) {
        return Err(ApiError::new(
            ERR_TRAY_UPDATE_FAILED,
            "tray labels must be non-empty",
        ));
    }

    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        log::error!("tray: set_tray_labels failed: tray not initialized");
        return Err(ApiError::new(
            ERR_TRAY_UPDATE_FAILED,
            "tray not initialized",
        ));
    };

    let show_item = MenuItem::with_id(&app, MENU_SHOW, show_main, true, None::<&str>)
        .map_err(tray_update_err)?;
    let quit_item = MenuItem::with_id(&app, MENU_QUIT, quit, true, None::<&str>)
        .map_err(tray_update_err)?;
    // 快速开关项：文案随 i18n 下发，勾选态保持当前设置（id 不变）
    let broadcast_item = CheckMenuItem::with_id(
        &app,
        MENU_BROADCAST,
        broadcast,
        true,
        hooks::lan_sync_broadcast_enabled().unwrap_or(true),
        None::<&str>,
    )
    .map_err(tray_update_err)?;
    let receive_item = CheckMenuItem::with_id(
        &app,
        MENU_RECEIVE,
        receive,
        true,
        hooks::lan_sync_receive_enabled().unwrap_or(true),
        None::<&str>,
    )
    .map_err(tray_update_err)?;

    let menu = Menu::with_items(
        &app,
        &[&show_item, &quit_item, &broadcast_item, &receive_item],
    )
    .map_err(tray_update_err)?;
    tray.set_menu(Some(menu)).map_err(tray_update_err)?;

    // 菜单重建后开关项句柄已变化，更新句柄供后续勾选态同步（id 不变）
    tray_items_slot().lock().unwrap().replace(TrayItems {
        broadcast: broadcast_item,
        receive: receive_item,
    });
    log::debug!("tray: labels updated");
    Ok(())
}

/// 切换「剪贴板广播」开关并同步勾选态。
fn toggle_broadcast(app: &AppHandle) {
    let current = hooks::lan_sync_broadcast_enabled().unwrap_or(true);
    let next = !current;
    match hooks::lan_sync_set_broadcast(app, next) {
        Some(Ok(value)) => {
            if let Some(items) = tray_items_slot().lock().unwrap().as_ref() {
                if let Err(e) = items.broadcast.set_checked(value) {
                    log::warn!("tray: sync broadcast checked state failed: {e}");
                }
            }
            log::info!("tray: broadcast toggled -> {value}");
        }
        Some(Err(e)) => log::error!("tray: broadcast toggle failed: {e}"),
        None => log::warn!("tray: broadcast toggle skipped (lan-sync not registered)"),
    }
}

/// 切换「剪贴板接收」开关并同步勾选态。
fn toggle_receive(app: &AppHandle) {
    let current = hooks::lan_sync_receive_enabled().unwrap_or(true);
    let next = !current;
    match hooks::lan_sync_set_receive(app, next) {
        Some(Ok(value)) => {
            if let Some(items) = tray_items_slot().lock().unwrap().as_ref() {
                if let Err(e) = items.receive.set_checked(value) {
                    log::warn!("tray: sync receive checked state failed: {e}");
                }
            }
            log::info!("tray: receive toggled -> {value}");
        }
        Some(Err(e)) => log::error!("tray: receive toggle failed: {e}"),
        None => log::warn!("tray: receive toggle skipped (lan-sync not registered)"),
    }
}

/// 托盘文案校验：去首尾空白后均非空才合法。
fn labels_are_valid(show: &str, quit: &str) -> bool {
    !show.trim().is_empty() && !quit.trim().is_empty()
}

fn tray_update_err(err: impl std::fmt::Display) -> ApiError {
    log::error!("{ERR_TRAY_UPDATE_FAILED}: {err}");
    ApiError::new(
        ERR_TRAY_UPDATE_FAILED,
        format!("failed to update tray menu: {err}"),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_valid_accepts_normal_labels() {
        assert!(labels_are_valid("显示主窗口", "退出"));
        assert!(labels_are_valid("Show Main Window", "Quit"));
    }

    #[test]
    fn labels_are_valid_rejects_empty_or_whitespace() {
        assert!(!labels_are_valid("", "退出"));
        assert!(!labels_are_valid("显示主窗口", ""));
        assert!(!labels_are_valid("   ", "   "));
    }

    #[test]
    fn labels_are_valid_trims_whitespace_before_checking() {
        assert!(labels_are_valid("  显示主窗口  ", "退出 "));
    }
}
