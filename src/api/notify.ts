//! 通知（notify）：前端唯一的 invoke 封装（docs/architecture.md 第 2 节）。
//!
//! 契约：`docs/api/notify.md`。
//!
//! 通道（双向统一，契约 1/5.1）：
//! - 前端调 `notify()`（fire-and-forget）→ 后端校验 → 广播 `app://notify` 事件；
//! - 后端内部站点（托盘开关失败、快捷键注册失败、lan-sync 节点错误等）直接 emit 同一事件；
//! - NotificationProvider 监听 `APP_NOTIFY_EVENT` 渲染（仅主窗口挂载）。
//!
//! 错误码解析（契约 5.4）：`code` 为稳定 i18n 键或后端错误码；后端错误码经
//! `resolveNotifyCode` 映射为 i18n 键（`lan.*` → `lanSync.*`、`quick_paste.*` → `quickPaste.*`），
//! 顺带修复「lan 错误码在设置页/收件箱静默消失」的现存 bug。

import { invoke } from "@tauri-apps/api/core";

/** 通知级别（与后端契约一致，序列化为小写字符串）。 */
export type NotifyLevel = "success" | "error" | "warning" | "info";

/** 通知事件名（后端 → 前端）。 */
export const APP_NOTIFY_EVENT = "app://notify";

/** 通知负载（与命令入参同构）。 */
export interface NotifyPayload {
  level: NotifyLevel;
  /** 稳定 i18n 键（如 `clipboard.copied`）或后端错误码（如 `lan.peer_node_error`）。 */
  code: string;
  /** 插值参数（可选）。 */
  params?: Record<string, string | number | boolean>;
}

/** 兜底通用键：code 查不到 i18n 文案时使用（带 `{code}` 参数，便于排查）。 */
export const UNKNOWN_NOTIFY_CODE = "notify.unknown";

/**
 * 后端错误码 → i18n 键映射表（契约 5.4）。
 *
 * 后端错误码为 `<domain>.<error>` 全小写点分命名（`lan.*`、`quick_paste.*`），
 * i18n 键域名为 camelCase（`lanSync.*`、`quickPaste.*`）——`clipboard.*` 两者一致，
 * 无需映射；其余在此补齐。
 */
const BACKEND_CODE_TO_I18N: Record<string, string> = {
  "quick_paste.register_failed": "quickPaste.register_failed",
  "quick_paste.tray_update_failed": "quickPaste.trayUpdateFailed",
  "lan.storage_error": "lanSync.storage_error",
  "lan.entry_not_found": "lanSync.entry_not_found",
  "lan.invalid_name": "lanSync.invalid_name",
  "lan.node_not_running": "lanSync.node_not_running",
  "lan.peer_node_error": "lanSync.peer_node_error",
  "lan.too_large": "lanSync.too_large",
};

/**
 * 解析通知 code 为 i18n 键：`t(code)` 直接命中 → 否则查映射表 → 否则返回原 code
 * （渲染层再兜底 `notify.unknown`）。返回空串表示 code 缺失。
 */
export function resolveNotifyCode(code: string): string {
  if (!code) return "";
  return BACKEND_CODE_TO_I18N[code] ?? code;
}

/**
 * 提交一条通知（fire-and-forget）。
 *
 * 后端校验 level/code，非法返回 `notify.invalid`（前端仅记日志，不再次通知——
 * 通知不该拖垮主流程）。成功时后端广播 `app://notify`，由 NotificationProvider 渲染。
 */
export async function notify(payload: NotifyPayload): Promise<void> {
  try {
    await invoke("notify", { ...payload });
  } catch (err) {
    console.warn("notify: submit failed:", err);
  }
}
