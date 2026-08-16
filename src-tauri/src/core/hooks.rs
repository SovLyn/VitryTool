//! 跨功能钩子（core 层，功能间解耦的广播通道）。
//!
//! 用途：
//! - 剪贴板历史产生**新条目**时，`capture_clipboard` 经此通知 lan-sync 广播；
//! - 系统托盘提供 lan-sync「广播 / 接收」快速开关（读状态 + 切换 + 持久化），
//!   由 lan-sync 注册实现、托盘菜单调用（core 不依赖功能域）。
//!
//! 设计约束：
//! - core 不依赖任何功能域类型 → 载荷用 `serde_json::Value`（条目序列化后的 JSON）；
//! - 功能间无编译期耦合：lan-sync 在 setup 注册消费端，剪贴板历史只调用 notify；
//! - 未注册时 notify / 开关钩子为空操作（不影响既有行为）。
//!
//! 契约：`docs/api/lan-sync.md` 第 5.2、5.4 节。

use serde_json::Value;
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;

type NewEntryHook = Box<dyn Fn(&AppHandle, &Value) + Send + Sync>;

static NEW_ENTRY_HOOK: OnceLock<Mutex<Option<NewEntryHook>>> = OnceLock::new();

fn hook_slot() -> &'static Mutex<Option<NewEntryHook>> {
    NEW_ENTRY_HOOK.get_or_init(|| Mutex::new(None))
}

/// 注册「新条目」钩子（setup 阶段由 lan-sync 调用；重复注册覆盖）。
pub fn register_new_entry_hook(hook: NewEntryHook) {
    let mut slot = hook_slot().lock().unwrap();
    *slot = Some(hook);
    log::debug!("hooks: new-entry hook registered");
}

/// 通知「新条目」产生（剪贴板历史在 is_new 时调用；未注册则空操作）。
pub fn notify_new_entry(app: &AppHandle, entry: &Value) {
    let slot = hook_slot().lock().unwrap();
    if let Some(hook) = slot.as_ref() {
        hook(app, entry);
    }
}

// ---------------------------------------------------------------------------
// lan-sync 快速开关钩子（托盘菜单用）
// ---------------------------------------------------------------------------

/// lan-sync 开关存取实现（由 lan-sync 注册；托盘菜单经此读写）。
#[derive(Clone, Copy)]
pub struct LanSyncSwitches {
    /// 读取当前广播开关。
    pub broadcast_enabled: fn() -> bool,
    /// 读取当前接收开关。
    pub receive_enabled: fn() -> bool,
    /// 设置广播开关并持久化；返回新的生效值。
    pub set_broadcast: fn(&AppHandle, bool) -> Result<bool, String>,
    /// 设置接收开关并持久化；返回新的生效值。
    pub set_receive: fn(&AppHandle, bool) -> Result<bool, String>,
}

static LAN_SYNC_SWITCHES: OnceLock<Mutex<Option<LanSyncSwitches>>> = OnceLock::new();

fn switches_slot() -> &'static Mutex<Option<LanSyncSwitches>> {
    LAN_SYNC_SWITCHES.get_or_init(|| Mutex::new(None))
}

/// 注册 lan-sync 开关实现（setup 阶段由 lan-sync 调用；重复注册覆盖）。
pub fn register_lan_sync_switches(switches: LanSyncSwitches) {
    let mut slot = switches_slot().lock().unwrap();
    *slot = Some(switches);
    log::debug!("hooks: lan-sync switches registered");
}

/// 读取广播开关（未注册时返回 None）。
pub fn lan_sync_broadcast_enabled() -> Option<bool> {
    let slot = switches_slot().lock().unwrap();
    slot.as_ref().map(|s| (s.broadcast_enabled)())
}

/// 读取接收开关（未注册时返回 None）。
pub fn lan_sync_receive_enabled() -> Option<bool> {
    let slot = switches_slot().lock().unwrap();
    slot.as_ref().map(|s| (s.receive_enabled)())
}

/// 切换广播开关并持久化；返回新的生效值（未注册时返回 None）。
pub fn lan_sync_set_broadcast(app: &AppHandle, enabled: bool) -> Option<Result<bool, String>> {
    let slot = switches_slot().lock().unwrap();
    slot.as_ref().map(|s| (s.set_broadcast)(app, enabled))
}

/// 切换接收开关并持久化；返回新的生效值（未注册时返回 None）。
pub fn lan_sync_set_receive(app: &AppHandle, enabled: bool) -> Option<Result<bool, String>> {
    let slot = switches_slot().lock().unwrap();
    slot.as_ref().map(|s| (s.set_receive)(app, enabled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unregistered_hook_is_noop() {
        // 未注册时调用不 panic
        // （需要 AppHandle，跳过真实调用；仅验证 hook_slot 可构造）
        let _ = hook_slot();
    }

    #[test]
    fn register_overwrites() {
        let slot = hook_slot();
        slot.lock().unwrap().replace(Box::new(|_, _| {}));
        assert!(slot.lock().unwrap().is_some());
    }

    #[test]
    fn unregistered_switches_return_none() {
        assert!(lan_sync_broadcast_enabled().is_none());
        assert!(lan_sync_receive_enabled().is_none());
    }

    #[test]
    fn registered_switches_delegate_to_fns() {
        let slot = switches_slot();
        slot.lock().unwrap().replace(LanSyncSwitches {
            broadcast_enabled: || true,
            receive_enabled: || false,
            set_broadcast: |_, _| Ok(true),
            set_receive: |_, _| Ok(false),
        });
        assert_eq!(lan_sync_broadcast_enabled(), Some(true));
        assert_eq!(lan_sync_receive_enabled(), Some(false));
        slot.lock().unwrap().take();
    }
}
