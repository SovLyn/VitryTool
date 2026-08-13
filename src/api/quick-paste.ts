//! 快速粘贴：前端唯一的 invoke 封装（docs/architecture.md 第 2 节）。
//!
//! 类型与命令与 `docs/api/quick-paste.md` 契约一致；错误码为 `quick_paste.*`，
//! 前端以错误码为 key 查 i18n 字典（见 `src/i18n/locales/*.json` 的 `quickPaste` 节）。
//!
//! 说明：`getErrorCode` 等通用工具复用 `src/api/clipboard-history.ts`。

import { invoke } from "@tauri-apps/api/core";

/** 读取当前全局快捷键（标准格式，如 "CommandOrControl+Shift+K"）；未设置返回 null。 */
export function getHotkey(): Promise<string | null> {
  return invoke<string | null>("get_hotkey");
}

/** 设置 / 清除全局快捷键并即时重注册（空串 = 清除）。 */
export function setHotkey(hotkey: string): Promise<void> {
  return invoke<void>("set_hotkey", { hotkey });
}

/** popup 前端加载完成握手：后端如有挂起的按下事件则补发 show。 */
export function quickPasteReady(): Promise<void> {
  return invoke<void>("quick_paste_ready");
}

/** popup 前端完成回写（或取消）后请求关闭（隐藏窗口、复位状态）。 */
export function quickPasteClose(sessionId: number): Promise<void> {
  return invoke<void>("quick_paste_close", { sessionId });
}
