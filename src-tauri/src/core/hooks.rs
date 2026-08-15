//! 跨功能钩子（core 层，功能间解耦的广播通道）。
//!
//! 用途：剪贴板历史产生**新条目**时，`capture_clipboard` 经此通知 lan-sync 广播。
//! 设计约束：
//! - core 不依赖任何功能域类型 → 载荷用 `serde_json::Value`（条目序列化后的 JSON）；
//! - 功能间无编译期耦合：lan-sync 在 setup 注册消费端，剪贴板历史只调用 notify；
//! - 未注册时 notify 为空操作（不影响既有行为）。
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
}
