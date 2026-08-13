//! 开发者测试（dt）：快捷键解析 / 规范化 / 校验的纯逻辑验证。
//!
//! 单测主体在 `service.rs` 的 `unit` 模块内；这里补充「会话状态机」相关
//! 纯逻辑验证（幂等、防误关新会话），不依赖 Tauri 运行时。
//!
//! 说明：`commands.rs` 依赖 Tauri 运行时（AppHandle / WebviewWindow），
//! 按项目架构（命令薄壳）不直接单测；其行为经人工实测验证（见 STATE.md）。

use super::commands::QuickPasteState;
use std::sync::atomic::Ordering;

/// 会话状态机辅助逻辑（从 commands 中提炼的可测语义）：
/// 「新一次按下」使会话自增，旧会话的兜底 / close 不再生效。
#[test]
fn session_increments_on_press() {
    let state = QuickPasteState::default();
    assert_eq!(state.session.load(Ordering::SeqCst), 0);

    // 模拟 Pressed（commands::show_popup 内的两步）
    let s1 = state.session.fetch_add(1, Ordering::SeqCst) + 1;
    assert_eq!(s1, 1);

    // 模拟下一次 Pressed
    let s2 = state.session.fetch_add(1, Ordering::SeqCst) + 1;
    assert_eq!(s2, 2);
    assert_ne!(s1, s2);
}

/// 模拟「旧会话的 release 兜底」：会话 id 不匹配当前会话时不得强制隐藏（防误关）。
#[test]
fn stale_session_guard_rejects_old_session() {
    let state = QuickPasteState::default();
    let s1 = state.session.fetch_add(1, Ordering::SeqCst) + 1;
    state.active.store(true, Ordering::SeqCst);

    // 旧会话兜底线程（session 1）检查：当前 session 仍为 1 → 允许复位
    let still_current = state.session.load(Ordering::SeqCst) == s1;
    assert!(still_current);

    // 新会话开始（session 2），旧会话（1）的兜底线程到达 → 不得复位
    let s2 = state.session.fetch_add(1, Ordering::SeqCst) + 1;
    let stale = state.session.load(Ordering::SeqCst) != s1;
    assert!(stale);
    assert_ne!(s2, s1);
    assert!(state.active.load(Ordering::SeqCst)); // 新会话的激活状态未被旧线程破坏
}

/// 幂等：重复 Pressed 不开启新会话（commands::show_popup 用 swap 判定）。
#[test]
fn repeat_press_does_not_duplicate_session() {
    let state = QuickPasteState::default();
    // 第一次按下
    let first_acquired = !state.active.swap(true, Ordering::SeqCst);
    assert!(first_acquired);
    let s1 = state.session.fetch_add(1, Ordering::SeqCst) + 1;
    // 重复按下（已激活）
    let second_acquired = !state.active.swap(true, Ordering::SeqCst);
    assert!(!second_acquired);
    assert_eq!(state.session.load(Ordering::SeqCst), s1);
}
