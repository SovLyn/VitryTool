//! 应用级剪贴板捕捉（listener）。
//!
//! 背景（修复 0.2.1 bug）：监听此前绑定在 `ClipboardHistory` 组件生命周期内，
//! 用户切到设置页（组件卸载）后监听停止，新复制的内容不会进入历史。
//! 现提升为**应用级**：`App` 挂载时启动一次，不随页面切换停止；
//! 主窗口隐藏后 WebView 仍存活（托盘常驻），监听与定时清理继续工作（ADR 0001 前提不变）。
//!
//! 职责：
//! - 启动即监听剪贴板变化 → `captureClipboard`（后端读/落盘/去重置顶/淘汰）；
//! - 捕捉到新条目后广播 `clipboard-history://updated`（主窗口历史页与小窗刷新）；
//! - 前台定时（5 分钟）发起 `cleanupOrphanImages` 兜底清理孤儿图片。

import { emit } from "@tauri-apps/api/event";
import { onClipboardChange, startListening } from "tauri-plugin-clipboard-x-api";
import { captureClipboard, cleanupOrphanImages, type ClipboardEntry } from "../../api/clipboard-history";

/** 剪贴板历史已更新的全局事件。 */
export const CLIPBOARD_UPDATED_EVENT = "clipboard-history://updated";

/**
 * 事件载荷：
 * - `entry` 存在（捕捉路径）：完整新条目（可能为去重置顶后的既有条目）→ 收件方**本地增量应用**，
 *   消除复制时的全量刷新（见 `incremental.ts`）；
 * - `entry` 缺失（收藏切换路径）：收件方退化为全量刷新（低频操作）。
 */
export interface ClipboardUpdatedEvent {
  id: string;
  entry?: ClipboardEntry;
}

/** 定时兜底清理间隔（固定，不暴露设置项，D6）。 */
const SWEEP_INTERVAL_MS = 5 * 60 * 1000;

let started = false;

/** 启动应用级剪贴板捕捉（幂等：重复调用只启动一次）。 */
export function startClipboardCapture(): void {
  if (started) return;
  started = true;

  void startListening()
    .then(() =>
      onClipboardChange(() => {
        void captureClipboard()
          .then((entry) => {
            if (entry) void emit(CLIPBOARD_UPDATED_EVENT, { id: entry.id, entry });
          })
          .catch(() => {
            // 捕捉失败不阻塞监听链路（后端已有日志）
          });
      }),
    )
    .catch(() => {
      // 启动失败允许下次重试（如 WebView 环境未就绪）
      started = false;
    });

  // 定时兜底清理：由前台发起（D6、ADR 0001）
  setInterval(() => {
    void cleanupOrphanImages().catch(() => {});
  }, SWEEP_INTERVAL_MS);
}
