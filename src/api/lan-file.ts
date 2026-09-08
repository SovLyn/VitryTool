//! 局域网文件共享（lan-file）：前端唯一的 invoke 封装（docs/architecture.md 第 2 节）。
//!
//! 类型与命令与 `docs/api/lan-file.md` 契约一致；错误码为 `lan_file.*`，
//! 前端以错误码为 key 查 i18n 字典（`src/i18n/locales/*.json` 的 `lanFile` 节）。

import { invoke } from "@tauri-apps/api/core";
import { appDataDir, join } from "@tauri-apps/api/path";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { getErrorCode } from "./clipboard-history";
import { resolveNotifyCode, UNKNOWN_NOTIFY_CODE } from "./notify";
import type { TFunction } from "../i18n";

// ---------------------------------------------------------------------------
// 类型（契约第 3 节）
// ---------------------------------------------------------------------------

/** `getLanFileStatus` 响应。 */
export interface LanFileStatus {
  enabled: boolean;
  listening: boolean;
  tcpPort: number | null;
  peerCount: number;
  trustedCount: number;
}

/** `getLanFilePeers` 元素。 */
export interface LanFilePeer {
  peerId: string;
  terminalName: string;
  fingerprint: string;
  trusted: boolean;
  supportsInteractive: boolean;
}

/** `getLanFilePeers` 响应。 */
export interface LanFilePeersResp {
  peers: LanFilePeer[];
}

/** `lan-file://incoming` 载荷。 */
export interface LanFileOffer {
  transferId: string;
  peerId: string;
  terminalName: string;
  fingerprint: string;
  /** true = 陌生 peerId 撞已信任终端名（红色警告卡，契约 5.4）。 */
  nameClash: boolean;
  files: { name: string; size: number }[];
  totalBytes: number;
  /** 该 peerId 上次出现在公告列表距今（分钟；TOFU 参考信息）。 */
  knownFromMinutes: number;
}

/** 文件进度行。 */
export interface LanTransferFile {
  name: string;
  size: number;
  transferredBytes: number;
  status: "pending" | "transferring" | "done";
}

/** `lan-file://transfer-updated` 载荷（活跃任务全量快照）。 */
export interface LanFileTransfer {
  /** undefined = 空闲（回到空闲时后端省略字段）。 */
  transferId?: string | null;
  direction: "send" | "receive";
  peerId?: string | null;
  terminalName?: string | null;
  state:
    | "offering"
    | "transferring"
    | "resuming"
    | "done"
    | "failed"
    | "cancelled"
    | "rejected";
  files: LanTransferFile[];
  totalBytes: number;
  transferredBytes: number;
  /** 后端 EMA；非传输态为 0。 */
  bytesPerSec: number;
  /** 仅接收侧 done：落盘最终路径。 */
  savedPaths?: string[];
  error?: { code: string; params?: Record<string, string | number> };
}

/** `sendLanFile` 响应。 */
export interface SendLanFileResp {
  transferId: string;
}

/** `checkLanFilePaths` 元素（拖入/选择后的路径预检）。 */
export interface LanFilePathInfo {
  path: string;
  /** 净化后的展示名。 */
  name: string;
  /** 字节数（非普通文件为 0）。 */
  size: number;
  /** 目录：v1 不支持传输（前端忽略并提示）。 */
  isDir: boolean;
  /** 普通文件且可读 = 可直接发送。 */
  readable: boolean;
}

/** `checkLanFilePaths` 响应。 */
export interface LanFilePathsResp {
  paths: LanFilePathInfo[];
}

// ---------------------------------------------------------------------------
// 事件名（契约第 2 节）
// ---------------------------------------------------------------------------

/** 公告终端上下线（收到后重拉 getLanFilePeers）。 */
export const LAN_FILE_PEERS_UPDATED_EVENT = "lan-file://peers-updated";
/** 新提议到达（resumed 续传不发，静默恢复）。 */
export const LAN_FILE_INCOMING_EVENT = "lan-file://incoming";
/** 活跃任务全量快照；transferId 空表示回到空闲。 */
export const LAN_FILE_TRANSFER_UPDATED_EVENT = "lan-file://transfer-updated";
/** 总开关变化（托盘 ⇄ 设置页双向同步）。 */
export const LAN_FILE_SETTINGS_UPDATED_EVENT = "lan-file://settings-updated";

// ---------------------------------------------------------------------------
// 命令封装
// ---------------------------------------------------------------------------

/** 总开关、监听状态与端口、公告可用终端数、已信任终端数。 */
export function getLanFileStatus(): Promise<LanFileStatus> {
  return invoke<LanFileStatus>("get_lan_file_status");
}

/** 可传终端列表（公告了 lan-file 能力的在线节点，含信任标记）。 */
export function getLanFilePeers(): Promise<LanFilePeersResp> {
  return invoke<LanFilePeersResp>("get_lan_file_peers");
}

/** 发起传输任务 → 返回 transferId；已有活跃任务报 `lan_file.busy`。 */
export function sendLanFile(peerId: string, filePaths: string[]): Promise<SendLanFileResp> {
  return invoke<SendLanFileResp>("send_lan_file", { peerId, filePaths });
}

/**
 * 路径预检（契约 5.6）：拖入/选择后先问后端「能不能传」。
 *
 * 用于把**目录**挡在待传列表之外（v1 不支持文件夹传输），并顺带拿到文件大小。
 */
export function checkLanFilePaths(filePaths: string[]): Promise<LanFilePathsResp> {
  return invoke<LanFilePathsResp>("check_lan_file_paths", { filePaths });
}

/** 接受提议（未知终端同时写入信任表 = TOFU 确认）。 */
export function acceptLanFile(transferId: string): Promise<void> {
  return invoke<void>("accept_lan_file", { transferId });
}

/** 拒绝提议（通知对端 + 清理，无状态残留）。 */
export function rejectLanFile(transferId: string): Promise<void> {
  return invoke<void>("reject_lan_file", { transferId });
}

/** 显式取消 = 永久终止（通知对端、删 .tmp 与 sidecar、写取消墓碑）。 */
export function cancelLanFileTransfer(transferId: string): Promise<void> {
  return invoke<void>("cancel_lan_file_transfer", { transferId });
}

/** 功能总开关（默认开；关闭 = 停监听、停公告、终止活跃任务、图片通道停）。 */
export function setLanFileEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("set_lan_file_enabled", { enabled });
}

/** 已信任终端列表（设置页）。 */
export interface TrustedPeer {
  peerId: string;
  terminalName: string;
  trustedAt: string;
}

/** 已信任终端列表。 */
export function getLanFileTrustedPeers(): Promise<TrustedPeer[]> {
  return invoke<TrustedPeer[]>("get_lan_file_trusted_peers");
}

/** 移除信任（下次该终端提议重新走 TOFU 弹窗）。 */
export function removeLanFileTrustedPeer(peerId: string): Promise<void> {
  return invoke<void>("remove_lan_file_trusted_peer", { peerId });
}

/** 后端 lan_file 错误码 → i18n 键（`lanFile.*` 域；错误码前缀即 lanFile，直接映射）。 */
export function lanFileError(err: unknown): string {
  const code = getErrorCode(err);
  return code || "lan_file.transfer_failed";
}

/**
 * 稳定错误码 → 可读文案（契约第 4 节）。
 *
 * 映射链：后端码 → `resolveNotifyCode` 的 i18n 键 → `t()`；查不到 → `notify.unknown`
 * （带 `{code}` 参数便于排查）。空码返回空串（调用方不渲染错误行）。
 */
export function translateLanFileError(t: TFunction, code?: string | null): string {
  if (!code) return "";
  const text = t(resolveNotifyCode(code), { code });
  return text || t(UNKNOWN_NOTIFY_CODE, { code });
}

/** 在文件管理器中显示落盘文件（契约 5.3：接收侧 done 的「打开所在位置」）。 */
export async function revealSavedPath(path: string): Promise<void> {
  await revealItemInDir(path);
}

/** 一键打开本机**接收文件夹**（`AppData/lanfile`，交互传输落盘目录，契约 5.5）。 */
export async function openReceiveFolder(): Promise<void> {
  await openPath(await join(await appDataDir(), "lanfile"));
}

/** 字节数人性化展示（KB/MB/GB，1 位小数；<1KB 显示 B）。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log2(bytes) / 10), units.length - 1);
  const v = bytes / 1024 ** i;
  return `${i === 0 ? v : v.toFixed(1)} ${units[i]}`;
}

/** 速率展示（bytes/sec → KB/s、MB/s）。 */
export function formatSpeed(bps: number): string {
  if (!Number.isFinite(bps) || bps <= 0) return "";
  return `${formatBytes(bps)}/s`;
}
