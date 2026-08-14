//! 剪贴板历史：前端唯一的 invoke 封装（docs/architecture.md 第 2 节）。
//!
//! 类型与命令与 `docs/api/clipboard-history.md` 契约一致；错误码为 `clipboard.*`，
//! 前端以错误码为 key 查 i18n 字典（见 `src/i18n/locales/*.json` 的 `clipboard` 节）。

import { invoke } from "@tauri-apps/api/core";

/** 图片引用（本体在插件默认目录，`missing` 为派生标记）。 */
export interface ClipboardImage {
  path: string;
  size: number;
  width: number;
  height: number;
  missing: boolean;
}

/** 文件引用（不复制本体，仅记录源路径）。 */
export interface ClipboardFiles {
  paths: string[];
  size: number;
}

/** 一条剪贴板历史记录（一次剪贴板变化 = 一条，字段共存）。 */
export interface ClipboardEntry {
  id: string;
  capturedAt: string; // ISO 8601
  favoritedAt?: string; // ISO 8601 收藏时刻；存在即收藏（0.2.4）
  text?: string;
  html?: string;
  rtf?: string;
  image?: ClipboardImage;
  files?: ClipboardFiles;
}

/** `cleanupOrphanImages` 响应。 */
export interface CleanupResp {
  removed: number;
}

/** `setMaxEntries` 响应。 */
export interface SetMaxResp {
  maxEntries: number;
  evicted: number;
}

/** invoke 拒绝值中的错误结构（后端 ApiError，code/message）。 */
export interface ApiInvokeError {
  code: string;
  message: string;
}

/** 从任意拒绝值中提取稳定错误码；非结构化错误时返回空串。 */
export function getErrorCode(err: unknown): string {
  if (typeof err === "object" && err !== null && "code" in err) {
    const code = (err as ApiInvokeError).code;
    return typeof code === "string" ? code : "";
  }
  return "";
}

/** 捕捉：监听事件触发（后端执行读剪贴板/落盘图片/去重置顶/即时淘汰）。空内容返回 null。 */
export function captureClipboard(): Promise<ClipboardEntry | null> {
  return invoke<ClipboardEntry | null>("capture_clipboard");
}

/** 读取全部条目（最新在前，图片缺失已标记）。 */
export function getClipboardHistory(): Promise<ClipboardEntry[]> {
  return invoke<ClipboardEntry[]>("get_clipboard_history");
}

/** 回写：按条目原始格式写回系统剪贴板。 */
export function writeClipboardEntry(id: string): Promise<void> {
  return invoke<void>("write_clipboard_entry", { id });
}

/** 单条删除（含其图片文件）。 */
export function deleteClipboardEntry(id: string): Promise<void> {
  return invoke<void>("delete_clipboard_entry", { id });
}

/** 设置/取消收藏（幂等；重复收藏刷新收藏时间，收藏区重新置顶）。 */
export function setEntryFavorite(id: string, favorited: boolean): Promise<void> {
  return invoke<void>("set_entry_favorite", { id, favorited });
}

/** 清空全部条目与图片文件。 */
export function clearClipboardHistory(): Promise<void> {
  return invoke<void>("clear_clipboard_history");
}

/** 定时兜底清理：删除无条目引用的孤儿图片。 */
export function cleanupOrphanImages(): Promise<CleanupResp> {
  return invoke<CleanupResp>("cleanup_orphan_images");
}

/** 读取当前上限 n。 */
export function getMaxEntries(): Promise<number> {
  return invoke<number>("get_max_entries");
}

/** 设置上限 n（1~1024），超限立即截断。 */
export function setMaxEntries(maxEntries: number): Promise<SetMaxResp> {
  return invoke<SetMaxResp>("set_max_entries", { maxEntries });
}
