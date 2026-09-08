//! 设置页测试：全局快捷键能力检测 → 警告 / 录制入口分支（0.2.3）。
//!
//! 覆盖契约 `docs/api/quick-paste.md` 5.8 的前端行为：
//! - `getHotkeyCapability` 返回 `supported=false`（如 Linux Wayland）→ 不渲染录制入口，显示警告；
//! - `supported=true` → 渲染录制入口，不显示警告。

import { cleanup, render, screen } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getMaxEntries } from "../../api/clipboard-history";
import { getLanFileTrustedPeers } from "../../api/lan-file";
import { getHotkey, getHotkeyCapability } from "../../api/quick-paste";
import { I18nProvider } from "../../i18n";
import { Settings } from "./Settings";

// ---- mocks ----
// theme 依赖 matchMedia（jsdom 未完整实现），此处打桩避免环境差异
vi.mock("../../theme", () => ({
  useTheme: () => ({ theme: () => "light", setTheme: vi.fn() }),
}));
vi.mock("../../api/clipboard-history", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/clipboard-history")>();
  return {
    ...actual,
    getMaxEntries: vi.fn(),
    setMaxEntries: vi.fn(),
  };
});
vi.mock("../../api/quick-paste", () => ({
  getHotkey: vi.fn(),
  setHotkey: vi.fn(),
  getHotkeyCapability: vi.fn(),
}));
vi.mock("../../api/lan-sync", () => ({
  getLanSyncStatus: vi.fn(async () => ({
    peerId: "test-peer",
    terminalName: "test-host",
    broadcastEnabled: true,
    receiveEnabled: true,
    nodeRunning: true,
    peerCount: 0,
  })),
  setLanSyncBroadcast: vi.fn(async () => undefined),
  setLanSyncReceive: vi.fn(async () => undefined),
  setLanSyncTerminalName: vi.fn(async () => undefined),
  LAN_INBOX_UPDATED_EVENT: "lan-sync://inbox-updated",
  LAN_SETTINGS_UPDATED_EVENT: "lan-sync://settings-updated",
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));
// lan-file（0.3.0）：信任列表 / 状态行
vi.mock("../../api/lan-file", () => ({
  getLanFileStatus: vi.fn(async () => ({
    enabled: true,
    listening: true,
    tcpPort: 60379,
    peerCount: 1,
    trustedCount: 1,
  })),
  getLanFileTrustedPeers: vi.fn(async () => []),
  setLanFileEnabled: vi.fn(async () => undefined),
  removeLanFileTrustedPeer: vi.fn(async () => undefined),
  LAN_FILE_SETTINGS_UPDATED_EVENT: "lan-file://settings-updated",
}));
// 通知系统（0.2.8）：设置操作反馈经全局通知；mock 掉 invoke 依赖
vi.mock("../../api/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/notify")>();
  return { ...actual, notify: vi.fn(async () => undefined) };
});

const mockedMaxEntries = vi.mocked(getMaxEntries);
const mockedGetHotkey = vi.mocked(getHotkey);
const mockedCapability = vi.mocked(getHotkeyCapability);
const mockedTrusted = vi.mocked(getLanFileTrustedPeers);

/** 等待 onMount 中的异步能力检测 / 数据拉取完成。 */
async function flush() {
  await Promise.resolve();
  await Promise.resolve();
  await new Promise((r) => setTimeout(r, 0));
}

async function renderSettings() {
  render(() => (
    <I18nProvider>
      <Settings />
    </I18nProvider>
  ));
  await flush();
}

describe("设置页 · 全局快捷键能力检测", () => {
  afterEach(() => cleanup());

  beforeEach(() => {
    mockedCapability.mockReset();
    mockedMaxEntries.mockResolvedValue(64);
    mockedGetHotkey.mockResolvedValue(null);
  });

  it("supported=false：不提供录制入口，显示平台警告", async () => {
    mockedCapability.mockResolvedValue({ supported: false });
    await renderSettings();

    // 警告文案（默认语言 zh-CN）
    expect(screen.getByText("全局快捷键在当前系统不可用")).toBeTruthy();
    expect(screen.getByText(/Wayland/)).toBeTruthy();
    // 录制入口（未设置按钮）不出现
    expect(screen.queryByText("未设置")).toBeNull();
  });

  it("supported=true：提供录制入口，不显示警告", async () => {
    mockedCapability.mockResolvedValue({ supported: true });
    await renderSettings();

    expect(screen.getByText("未设置")).toBeTruthy();
    expect(screen.queryByText("全局快捷键在当前系统不可用")).toBeNull();
  });

  it("能力检测失败：fail-open 为可用（不打扰正常环境用户）", async () => {
    mockedCapability.mockRejectedValue(new Error("invoke failed"));
    await renderSettings();

    expect(screen.getByText("未设置")).toBeTruthy();
    expect(screen.queryByText("全局快捷键在当前系统不可用")).toBeNull();
  });
});

describe("设置页 · 已信任终端列表（lan-file 0.3.0）", () => {
  afterEach(() => cleanup());

  beforeEach(() => {
    mockedCapability.mockResolvedValue({ supported: true });
    mockedMaxEntries.mockResolvedValue(64);
    mockedGetHotkey.mockResolvedValue(null);
    mockedTrusted.mockReset();
  });

  it("有已信任终端：渲染列表且**不显示空态提示**（状态行「已信任 1」与列表一致）", async () => {
    mockedTrusted.mockResolvedValue([
      {
        peerId: "12D3KooWQKXPQZEfk3CPM63QoHTbwvB1pT6F1qagAuKKDxJLHQ3u",
        terminalName: "SOVLYN",
        trustedAt: "2026-09-08T10:00:48Z",
      },
    ]);
    await renderSettings();

    expect(screen.getByText("SOVLYN")).toBeTruthy();
    expect(screen.getByText("12D3KooWQK…")).toBeTruthy();
    // 空态提示不得出现（真机实测 bug：它与列表同时显示）
    expect(screen.queryByText(/尚无已信任终端/)).toBeNull();
    expect(screen.getByText("移除信任")).toBeTruthy();
  });

  it("无已信任终端：显示空态提示", async () => {
    mockedTrusted.mockResolvedValue([]);
    await renderSettings();

    expect(screen.getByText(/尚无已信任终端/)).toBeTruthy();
    expect(screen.queryByText("移除信任")).toBeNull();
  });
});
