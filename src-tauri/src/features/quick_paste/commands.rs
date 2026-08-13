//! 快速粘贴命令（薄壳：参数解析 + 状态获取，业务逻辑在 service / store）。
//!
//! 职责（契约见 `docs/api/quick-paste.md`）：
//! - `getHotkey` / `setHotkey`：快捷键设置（校验、重注册、持久化）；
//! - `quickPasteReady`：popup 前端加载完成握手（补发挂起的 show 事件）；
//! - `quickPasteClose`：popup 前端完成回写（或取消）后关闭小屏；
//! - 全局快捷键 Pressed / Released 事件处理：show / release 小屏（本模块内部，不暴露为命令）。

use super::service::normalize_hotkey;
use super::store::HotkeyStore;
use crate::core::error::ApiError;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use tauri::{App, AppHandle, Emitter, Manager, PhysicalPosition, Position, WebviewWindow, Window, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

/// 小屏窗口 label（与 tauri.conf.json 中 quick-paste 窗口一致）。
pub const POPUP_LABEL: &str = "quick-paste";

/// 事件名（后端 → popup 前端）。
const EVENT_SHOW: &str = "quick-paste://show";
const EVENT_RELEASE: &str = "quick-paste://release";

/// 松开快捷键后，前端未及时关闭小屏的兜底隐藏时间（秒）。
const FORCE_HIDE_TIMEOUT_SECS: u64 = 3;

/// 错误码（契约第 4 节）。
const ERR_INVALID_HOTKEY: &str = "quick_paste.invalid_hotkey";
const ERR_REGISTER_FAILED: &str = "quick_paste.register_failed";

/// 事件载荷：会话 id（用于区分连续多次按下的会话，防止过期回调误关闭新会话）。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPayload {
    pub session: u32,
}

/// 快速粘贴运行状态（功能私有，挂载于 `init`）。
#[derive(Debug, Default)]
pub struct QuickPasteState {
    /// 当前已注册的快捷键（标准格式字符串）；`None` = 未注册。
    pub current_hotkey: Mutex<Option<String>>,
    /// 小屏当前是否处于激活会话（按下后未正常关闭）。
    pub active: AtomicBool,
    /// popup WebView 是否已加载完成（ready 握手，见契约 5.3）。
    pub popup_ready: AtomicBool,
    /// 是否有挂起的按下事件（popup 未 ready 时按下，待 ready 后补发 show）。
    pub pending_show: AtomicBool,
    /// 会话自增 id：每次按下递增；用于校验 release 兜底 / close 是否为当前会话。
    pub session: AtomicU32,
}

fn invalid_hotkey_err(err: impl std::fmt::Display) -> ApiError {
    log::warn!("{ERR_INVALID_HOTKEY}: {err}");
    ApiError::new(ERR_INVALID_HOTKEY, format!("invalid hotkey: {err}"))
}

fn register_failed_err(err: impl std::fmt::Display) -> ApiError {
    log::error!("{ERR_REGISTER_FAILED}: {err}");
    ApiError::new(
        ERR_REGISTER_FAILED,
        format!("failed to register global shortcut: {err}"),
    )
}

/// 初始化：挂载状态 + 注册已保存的快捷键（setup 阶段调用）。
pub fn init(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    app.manage(QuickPasteState::default());

    let handle = app.handle();
    let store = HotkeyStore::new(handle)?;
    if let Some(saved) = store.load_hotkey()? {
        match normalize_hotkey(&saved) {
            Ok(normalized) => {
                handle.global_shortcut().on_shortcut(normalized.as_str(), hotkey_handler)?;
                handle
                    .state::<QuickPasteState>()
                    .current_hotkey
                    .lock()
                    .unwrap()
                    .replace(normalized.clone());
                log::info!("quick_paste: registered saved global shortcut {normalized}");
            }
            Err(e) => log::error!("quick_paste: stored hotkey invalid ({e}), skip registration"),
        }
    }
    Ok(())
}

/// 读取当前快捷键；未设置返回 `null`。
#[tauri::command]
pub fn get_hotkey(app: AppHandle) -> Result<Option<String>, ApiError> {
    let store = HotkeyStore::new(&app)?;
    let hotkey = store.load_hotkey()?;
    log::debug!("get_hotkey: {hotkey:?}");
    Ok(hotkey)
}

/// 设置 / 清除全局快捷键（空串 = 清除），并即时重注册（契约 5.2）。
///
/// 注册失败时**不持久化**，并回滚恢复旧注册。
#[tauri::command]
pub fn set_hotkey(app: AppHandle, hotkey: String) -> Result<(), ApiError> {
    if hotkey.trim().is_empty() {
        return clear_hotkey(&app);
    }

    let normalized = normalize_hotkey(&hotkey).map_err(invalid_hotkey_err)?;
    let state = app.state::<QuickPasteState>();
    let shortcuts = app.global_shortcut();

    // 注销旧快捷键（失败忽略：可能从未注册成功过）
    let previous = state.current_hotkey.lock().unwrap().clone();
    if let Some(prev_str) = &previous {
        if let Err(e) = shortcuts.unregister(prev_str.as_str()) {
            log::debug!("set_hotkey: unregister previous {prev_str} failed: {e}");
        }
    }

    // 注册新快捷键；失败则回滚恢复旧注册
    if let Err(e) = shortcuts.on_shortcut(normalized.as_str(), hotkey_handler) {
        if let Some(prev_str) = &previous {
            let _ = shortcuts.on_shortcut(prev_str.as_str(), hotkey_handler);
        }
        return Err(register_failed_err(e));
    }

    *state.current_hotkey.lock().unwrap() = Some(normalized.clone());
    HotkeyStore::new(&app)?.save_hotkey(&normalized)?;
    log::info!("set_hotkey: registered {normalized}");
    Ok(())
}

/// 清除快捷键：注销 + 存储置空。
fn clear_hotkey(app: &AppHandle) -> Result<(), ApiError> {
    let state = app.state::<QuickPasteState>();
    let previous = state.current_hotkey.lock().unwrap().take();
    if let Some(prev_str) = previous {
        if let Err(e) = app.global_shortcut().unregister(prev_str.as_str()) {
            log::debug!("clear_hotkey: unregister {prev_str} failed: {e}");
        }
        log::info!("clear_hotkey: unregistered {prev_str}");
    }
    HotkeyStore::new(app)?.clear_hotkey()
}

/// popup 前端加载完成握手：若存在挂起的按下事件则补发 show（契约 5.3 竞态处理）。
#[tauri::command]
pub fn quick_paste_ready(app: AppHandle) -> Result<(), ApiError> {
    let state = app.state::<QuickPasteState>();
    state.popup_ready.store(true, Ordering::SeqCst);
    if state.pending_show.swap(false, Ordering::SeqCst) {
        log::debug!("quick_paste_ready: flush pending show");
        if let Some(popup) = app.get_webview_window(POPUP_LABEL) {
            let session = state.session.load(Ordering::SeqCst);
            let _ = popup.emit(EVENT_SHOW, SessionPayload { session });
        }
    }
    Ok(())
}

/// popup 前端完成回写（或取消）后请求关闭：隐藏小屏、复位状态（契约 5.3）。
///
/// `session_id` 必须与当前会话一致，防止过期回调误关新会话。
#[tauri::command]
pub fn quick_paste_close(app: AppHandle, session_id: u32) -> Result<(), ApiError> {
    let state = app.state::<QuickPasteState>();
    if state.session.load(Ordering::SeqCst) != session_id {
        log::debug!("quick_paste_close: stale session {session_id}, ignore");
        return Ok(());
    }
    state.active.store(false, Ordering::SeqCst);
    state.pending_show.store(false, Ordering::SeqCst);
    if let Some(popup) = app.get_webview_window(POPUP_LABEL) {
        if let Err(e) = popup.hide() {
            log::warn!("quick_paste_close: hide popup failed: {e}");
        } else {
            log::debug!("quick_paste_close: popup hidden (session {session_id})");
        }
    }
    Ok(())
}

/// 全局快捷键回调：Pressed 显示小屏，Released 通知回写并关闭。
fn hotkey_handler(app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent) {
    match event.state() {
        ShortcutState::Pressed => show_popup(app),
        ShortcutState::Released => release_popup(app),
    }
}

/// 快捷键按下：激活会话 → 定位到鼠标附近 → 显示并聚焦 → 通知前端（契约 5.3）。
fn show_popup(app: &AppHandle) {
    let state = app.state::<QuickPasteState>();
    if state.active.swap(true, Ordering::SeqCst) {
        log::debug!("quick-paste: already active, ignore repeat press");
        return;
    }
    let session = state.session.fetch_add(1, Ordering::SeqCst) + 1;
    state.pending_show.store(true, Ordering::SeqCst);
    log::debug!("quick-paste: pressed, session {session}");

    let Some(popup) = app.get_webview_window(POPUP_LABEL) else {
        log::error!("quick-paste: popup window not found");
        state.active.store(false, Ordering::SeqCst);
        state.pending_show.store(false, Ordering::SeqCst);
        return;
    };

    position_near_cursor(&popup);
    let _ = popup.show();
    let _ = popup.set_focus();

    if state.popup_ready.load(Ordering::SeqCst) {
        state.pending_show.store(false, Ordering::SeqCst);
        let _ = popup.emit(EVENT_SHOW, SessionPayload { session });
    } else {
        log::debug!("quick-paste: popup not ready yet, show pending");
    }
}

/// 快捷键松开：通知前端回写选中项；兜底 3 秒强制隐藏（契约 5.3）。
fn release_popup(app: &AppHandle) {
    let state = app.state::<QuickPasteState>();
    if !state.active.load(Ordering::SeqCst) && !state.pending_show.load(Ordering::SeqCst) {
        log::debug!("quick-paste: release without active session, ignore");
        return;
    }

    let Some(popup) = app.get_webview_window(POPUP_LABEL) else {
        state.active.store(false, Ordering::SeqCst);
        state.pending_show.store(false, Ordering::SeqCst);
        return;
    };

    let session = state.session.load(Ordering::SeqCst);
    if state.popup_ready.load(Ordering::SeqCst) {
        let _ = popup.emit(EVENT_RELEASE, SessionPayload { session });
        log::debug!("quick-paste: released, session {session}, wait for frontend");
    } else {
        // popup 未加载完成：无法回写，直接复位隐藏
        log::warn!("quick-paste: released before popup ready, hide directly");
        state.active.store(false, Ordering::SeqCst);
        state.pending_show.store(false, Ordering::SeqCst);
        let _ = popup.hide();
    }

    // 兜底：前端未在时限内关闭（回写卡住 / WebView 异常）则强制隐藏。
    let app2 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(FORCE_HIDE_TIMEOUT_SECS));
        force_hide_if_active(&app2, session);
    });
}

/// 兜底关闭：仅当仍处于 `session` 对应会话时生效（防旧线程误关新会话）。
fn force_hide_if_active(app: &AppHandle, session: u32) {
    let state = app.state::<QuickPasteState>();
    if state.session.load(Ordering::SeqCst) != session {
        return;
    }
    if state.active.swap(false, Ordering::SeqCst) {
        state.pending_show.store(false, Ordering::SeqCst);
        log::warn!("quick-paste: force-hid popup after timeout (session {session})");
        if let Some(popup) = app.get_webview_window(POPUP_LABEL) {
            let _ = popup.hide();
        }
    }
}

/// 将小屏定位到鼠标光标右下方，并 clamp 在当前显示器物理边界内（契约 5.4）。
fn position_near_cursor(popup: &WebviewWindow) {
    let Ok(cursor) = popup.cursor_position() else {
        return;
    };
    let Ok(size) = popup.outer_size() else {
        return;
    };

    const MARGIN: f64 = 24.0;
    let mut x = cursor.x + MARGIN;
    let mut y = cursor.y + MARGIN;

    if let Ok(Some(monitor)) = popup.monitor_from_point(cursor.x, cursor.y) {
        let mpos = monitor.position();
        let msize = monitor.size();
        let max_x = mpos.x + msize.width as i32 - size.width as i32;
        let max_y = mpos.y + msize.height as i32 - size.height as i32;
        x = x.clamp(mpos.x as f64, max_x.max(mpos.x) as f64);
        y = y.clamp(mpos.y as f64, max_y.max(mpos.y) as f64);
    }

    let _ = popup.set_position(Position::Physical(PhysicalPosition::new(x as i32, y as i32)));
    log::trace!("quick-paste: popup positioned at ({x:.0}, {y:.0})");
}

/// 窗口事件：主窗口与 popup 窗口的关闭都改为隐藏（托盘常驻，契约 5.5）。
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        log::debug!("window '{}' close requested -> hide to tray", window.label());
    }
}
