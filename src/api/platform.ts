//! 平台识别：前端唯一的 invoke 封装（docs/api/mobile.md 契约）。
//!
//! `getPlatformInfo` 是移动端功能隔离的唯一依据（契约 mobile 5.1）：
//! 前端启动时调用一次，据此隐藏/禁用移动端不存在的功能
//! （剪贴板监听、快速粘贴、托盘、广播开关等）。

import { invoke } from "@tauri-apps/api/core";

/** `getPlatformInfo` 响应（契约 mobile 3）。 */
export interface PlatformInfo {
  /** 是否移动平台（android / ios）。 */
  isMobile: boolean;
  /** 平台名："windows" | "macos" | "linux" | "android" | "ios"。 */
  platform: "windows" | "macos" | "linux" | "android" | "ios" | "unknown";
  /** 全局快捷键能力（移动端恒为 false，契约 mobile 5.1）。 */
  hotkeyCapability: {
    supported: boolean;
  };
}

/** 当前平台信息（惰性缓存：启动后不变）。 */
let cached: PlatformInfo | null = null;

/** 读取平台信息（惰性缓存，失败时按桌面 fail-open 处理，不阻塞功能）。 */
export async function getPlatformInfo(): Promise<PlatformInfo> {
  if (cached) return cached;
  const info = await invoke<PlatformInfo>("get_platform_info");
  cached = info;
  return info;
}

/** 当前是否移动端（快速同步判断；未加载时返回 false，调用方应优先 await getPlatformInfo）。 */
export function isMobilePlatform(): boolean {
  return cached?.isMobile ?? false;
}
