//! 局域网同步（lan-sync）：前端唯一的 invoke 封装（docs/architecture.md 第 2 节）。
//!
//! 类型与命令与 `docs/api/lan-sync.md` 契约一致；错误码为 `lan.*`，
//! 前端以错误码为 key 查 i18n 字典（`src/i18n/locales/*.json` 的 `lanSync` 节）。

import { invoke } from "@tauri-apps/api/core";

/** 图片元数据（首版仅元数据；字节传输 TODO）。 */
export interface LanImageMeta {
  name: string;
  width?: number;
  height?: number;
  size?: number;
}

/** `getLanSyncStatus` 响应。 */
export interface LanSyncStatus {
  peerId: string;
  terminalName: string;
  broadcastEnabled: boolean;
  receiveEnabled: boolean;
  nodeRunning: boolean;
  peerCount: number;
}

/** 收件箱条目。 */
export interface LanInboxEntry {
  id: string;
  peerId: string;
  terminalName: string;
  receivedAt: string; // ISO 8601（排序键）
  sentAt: string; // ISO 8601（展示）
  text?: string;
  html?: string;
  rtf?: string;
  filePaths?: string[];
  imageMeta?: LanImageMeta;
  fingerprint: string;
}

/** 收件箱分组：一个来源节点一桶。 */
export interface LanInboxNode {
  peerId: string;
  terminalName: string;
  entries: LanInboxEntry[];
}

/** `getLanInbox` 响应（后端 InboxData）。 */
export interface LanInboxResp {
  nodes: LanInboxNode[];
}

/** 收件箱变化通知事件（后端 → 前端）。 */
export const LAN_INBOX_UPDATED_EVENT = "lan-sync://inbox-updated";

/** 设置变化通知事件（0.2.7）：托盘/设置页切换广播/接收后触发，前端据此刷新开关状态。 */
export const LAN_SETTINGS_UPDATED_EVENT = "lan-sync://settings-updated";

/** 节点状态。 */
export function getLanSyncStatus(): Promise<LanSyncStatus> {
  return invoke<LanSyncStatus>("get_lan_sync_status");
}

/** 开/关广播。 */
export function setLanSyncBroadcast(enabled: boolean): Promise<void> {
  return invoke<void>("set_lan_sync_broadcast", { enabled });
}

/** 开/关接收。 */
export function setLanSyncReceive(enabled: boolean): Promise<void> {
  return invoke<void>("set_lan_sync_receive", { enabled });
}

/** 设置终端名（非空且 ≤ 32 字符）。 */
export function setLanSyncTerminalName(name: string): Promise<void> {
  return invoke<void>("set_lan_sync_terminal_name", { name });
}

/** 收件箱全量（节点按最新条目倒序，桶内按接收时间倒序）。 */
export function getLanInbox(): Promise<LanInboxResp> {
  return invoke<LanInboxResp>("get_lan_inbox");
}

/** 回写：按原格式写系统剪贴板 → 进本地历史（不重广播）。 */
export function writeLanInboxEntry(id: string): Promise<void> {
  return invoke<void>("write_lan_inbox_entry", { id });
}

/** 单条删除。 */
export function deleteLanInboxEntry(id: string): Promise<void> {
  return invoke<void>("delete_lan_inbox_entry", { id });
}

/** 清空收件箱。 */
export function clearLanInbox(): Promise<void> {
  return invoke<void>("clear_lan_inbox");
}
