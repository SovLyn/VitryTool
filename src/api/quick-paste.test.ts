import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  getHotkey,
  getHotkeyCapability,
  quickPasteClose,
  quickPasteReady,
  setHotkey,
} from "./quick-paste";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockedInvoke = vi.mocked(invoke);

describe("quick-paste api 封装", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("getHotkey 调用 get_hotkey，未设置返回 null", async () => {
    mockedInvoke.mockResolvedValueOnce(null);
    expect(await getHotkey()).toBeNull();
    expect(mockedInvoke).toHaveBeenCalledWith("get_hotkey");

    mockedInvoke.mockResolvedValueOnce("CommandOrControl+Shift+K");
    expect(await getHotkey()).toBe("CommandOrControl+Shift+K");
  });

  it("setHotkey 传递快捷键字符串", async () => {
    mockedInvoke.mockResolvedValue(undefined);
    await setHotkey("CommandOrControl+Shift+K");
    expect(mockedInvoke).toHaveBeenCalledWith("set_hotkey", { hotkey: "CommandOrControl+Shift+K" });

    await setHotkey("");
    expect(mockedInvoke).toHaveBeenCalledWith("set_hotkey", { hotkey: "" });
  });

  it("quickPasteReady / quickPasteClose 调用与参数", async () => {
    mockedInvoke.mockResolvedValue(undefined);
    await quickPasteReady();
    expect(mockedInvoke).toHaveBeenCalledWith("quick_paste_ready");

    await quickPasteClose(3);
    expect(mockedInvoke).toHaveBeenCalledWith("quick_paste_close", { sessionId: 3 });
  });

  it("getHotkeyCapability 调用 get_hotkey_capability 并透传 supported", async () => {
    mockedInvoke.mockResolvedValueOnce({ supported: false });
    expect(await getHotkeyCapability()).toEqual({ supported: false });
    expect(mockedInvoke).toHaveBeenCalledWith("get_hotkey_capability");

    mockedInvoke.mockResolvedValueOnce({ supported: true });
    expect(await getHotkeyCapability()).toEqual({ supported: true });
  });
});
