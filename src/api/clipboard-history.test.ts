import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  captureClipboard,
  cleanupOrphanImages,
  clearClipboardHistory,
  deleteClipboardEntry,
  getClipboardHistory,
  getErrorCode,
  getMaxEntries,
  setEntryFavorite,
  setMaxEntries,
  writeClipboardEntry,
} from "./clipboard-history";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockedInvoke = vi.mocked(invoke);

describe("clipboard-history api 封装", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("captureClipboard 调用 capture_clipboard，空内容返回 null", async () => {
    mockedInvoke.mockResolvedValue(null);
    const result = await captureClipboard();
    expect(mockedInvoke).toHaveBeenCalledWith("capture_clipboard");
    expect(result).toBeNull();
  });

  it("getClipboardHistory 调用 get_clipboard_history", async () => {
    mockedInvoke.mockResolvedValue([]);
    await getClipboardHistory();
    expect(mockedInvoke).toHaveBeenCalledWith("get_clipboard_history");
  });

  it("writeClipboardEntry / deleteClipboardEntry 传递 id", async () => {
    mockedInvoke.mockResolvedValue(undefined);
    await writeClipboardEntry("id-1");
    expect(mockedInvoke).toHaveBeenCalledWith("write_clipboard_entry", { id: "id-1" });

    await deleteClipboardEntry("id-2");
    expect(mockedInvoke).toHaveBeenCalledWith("delete_clipboard_entry", { id: "id-2" });
  });

  it("setEntryFavorite 传递 id 与目标状态（幂等显式设置）", async () => {
    mockedInvoke.mockResolvedValue(undefined);
    await setEntryFavorite("id-1", true);
    expect(mockedInvoke).toHaveBeenCalledWith("set_entry_favorite", { id: "id-1", favorited: true });

    await setEntryFavorite("id-1", false);
    expect(mockedInvoke).toHaveBeenCalledWith("set_entry_favorite", { id: "id-1", favorited: false });
  });

  it("clearClipboardHistory / cleanupOrphanImages 无参调用", async () => {
    mockedInvoke.mockResolvedValue({ removed: 3 });
    await clearClipboardHistory();
    expect(mockedInvoke).toHaveBeenCalledWith("clear_clipboard_history");

    const resp = await cleanupOrphanImages();
    expect(mockedInvoke).toHaveBeenCalledWith("cleanup_orphan_images");
    expect(resp.removed).toBe(3);
  });

  it("getMaxEntries / setMaxEntries 调用与参数", async () => {
    mockedInvoke.mockResolvedValueOnce(64);
    const n = await getMaxEntries();
    expect(mockedInvoke).toHaveBeenCalledWith("get_max_entries");
    expect(n).toBe(64);

    mockedInvoke.mockResolvedValueOnce({ maxEntries: 32, evicted: 1 });
    const resp = await setMaxEntries(32);
    expect(mockedInvoke).toHaveBeenCalledWith("set_max_entries", { maxEntries: 32 });
    expect(resp).toEqual({ maxEntries: 32, evicted: 1 });
  });

  it("getErrorCode 提取结构化错误码，非结构化值返回空串", () => {
    expect(
      getErrorCode({ code: "clipboard.entry_not_found", message: "x" }),
    ).toBe("clipboard.entry_not_found");
    expect(getErrorCode("plain error")).toBe("");
    expect(getErrorCode(null)).toBe("");
    expect(getErrorCode(undefined)).toBe("");
    expect(getErrorCode({ code: 42 })).toBe("");
  });
});
