import { fireEvent, render, screen, cleanup } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emit, listen } from "@tauri-apps/api/event";
import {
  captureClipboard,
  getClipboardHistory,
  setEntryFavorite,
  writeClipboardEntry,
} from "../../api/clipboard-history";
import { quickPasteClose, quickPasteReady } from "../../api/quick-paste";
import { I18nProvider } from "../../i18n";
import { CLIPBOARD_UPDATED_EVENT } from "../clipboard-history/listener";
import { clampIndex, QuickPastePopup, type SessionPayload } from "./QuickPastePopup";

// ---- mocks ----
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (p: string) => `asset://${p}`,
}));
vi.mock("../../api/clipboard-history", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/clipboard-history")>();
  return {
    ...actual,
    captureClipboard: vi.fn(),
    getClipboardHistory: vi.fn(),
    writeClipboardEntry: vi.fn(),
    setEntryFavorite: vi.fn(),
  };
});
vi.mock("../../api/quick-paste", () => ({
  quickPasteReady: vi.fn(),
  quickPasteClose: vi.fn(),
}));

const mockedListen = vi.mocked(listen);
const mockedEmit = vi.mocked(emit);
const mockedCapture = vi.mocked(captureClipboard);
const mockedGetHistory = vi.mocked(getClipboardHistory);
const mockedWrite = vi.mocked(writeClipboardEntry);
const mockedSetFavorite = vi.mocked(setEntryFavorite);
const mockedClose = vi.mocked(quickPasteClose);
const mockedReady = vi.mocked(quickPasteReady);

/** 收集事件监听器（组件 onMount 注册后触发）。 */
const handlers = new Map<string, (e: { payload: SessionPayload }) => void>();

const TEXT_ENTRY = { id: "e1", capturedAt: "2026-08-13T00:00:00Z", text: "第一条文本" };
const IMG_ENTRY = {
  id: "e2",
  capturedAt: "2026-08-13T01:00:00Z",
  image: { path: "/img/x.png", size: 1, width: 1, height: 1, missing: false },
};

/** 等待异步（listen 注册、内部 await、进入动画双 rAF）完成。 */
async function flush() {
  await Promise.resolve();
  await Promise.resolve();
  await new Promise((r) => setTimeout(r, 0));
  await new Promise<void>((r) => requestAnimationFrame(() => r()));
  await new Promise<void>((r) => requestAnimationFrame(() => r()));
}

async function renderPopup() {
  render(() => (
    <I18nProvider>
      <QuickPastePopup />
    </I18nProvider>
  ));
  await flush();
}

function fireShow(session = 1) {
  handlers.get("quick-paste://show")?.({ payload: { session } });
}

function fireRelease(session = 1) {
  handlers.get("quick-paste://release")?.({ payload: { session } });
}

describe("clampIndex 选中边界收敛", () => {
  it("clamp 到 [0, len-1]，空列表返回 0", () => {
    expect(clampIndex(0, 3)).toBe(0);
    expect(clampIndex(-1, 3)).toBe(0);
    expect(clampIndex(2, 3)).toBe(2);
    expect(clampIndex(5, 3)).toBe(2);
    expect(clampIndex(0, 0)).toBe(0);
  });
});

describe("QuickPastePopup 快速粘贴小屏", () => {
  afterEach(() => cleanup());

  beforeEach(() => {
    handlers.clear();
    mockedListen.mockReset();
    mockedListen.mockImplementation((event: string, handler: any) => {
      handlers.set(event, handler);
      return Promise.resolve(() => handlers.delete(event));
    });
    mockedReady.mockReset();
    mockedReady.mockResolvedValue(undefined);
    mockedClose.mockReset();
    mockedClose.mockResolvedValue(undefined);
    mockedWrite.mockReset();
    mockedWrite.mockResolvedValue(undefined);
    mockedSetFavorite.mockReset();
    mockedSetFavorite.mockResolvedValue(undefined);
    mockedEmit.mockReset();
    mockedCapture.mockReset();
    mockedCapture.mockResolvedValue(null);
    mockedGetHistory.mockReset();
    mockedGetHistory.mockResolvedValue([TEXT_ENTRY, IMG_ENTRY]);
  });

  it("挂载即握手 ready，并注册 show/release 监听", async () => {
    await renderPopup();
    expect(mockedReady).toHaveBeenCalled();
    expect(handlers.has("quick-paste://show")).toBe(true);
    expect(handlers.has("quick-paste://release")).toBe(true);
  });

  it("show 后先补捕捉再拉取历史并高亮第一项（最新）", async () => {
    await renderPopup();
    fireShow(1);
    await flush();

    // 补捕捉在前（主窗口可能在设置页/隐藏期间未捕捉到最新复制）
    expect(mockedCapture).toHaveBeenCalled();
    expect(mockedGetHistory).toHaveBeenCalled();
    const items = screen.getAllByRole("listitem");
    expect(items.length).toBe(2);
    expect(items[0].classList.contains("active")).toBe(true);
    expect(items[0].textContent).toContain("第一条文本");
  });

  it("激活期间收到 clipboard-history://updated → 实时刷新并保持选中条目", async () => {
    await renderPopup();
    fireShow(1);
    await flush();

    // 选中第二项，然后模拟主窗口捕捉到新内容并广播
    fireEvent.wheel(document.querySelector(".qp-card")!, { deltaY: 100 });
    await flush();
    expect(screen.getAllByRole("listitem")[1].classList.contains("active")).toBe(true);

    // 新列表：最新条目插到顶部，原选中条目（e2 图片）仍在
    mockedGetHistory.mockResolvedValue([
      { id: "e0", capturedAt: "2026-08-13T02:00:00Z", text: "新复制的内容" },
      TEXT_ENTRY,
      IMG_ENTRY,
    ]);
    handlers.get(CLIPBOARD_UPDATED_EVENT)?.({
      payload: { id: "e0" } as unknown as SessionPayload,
    });
    await flush();

    const items = screen.getAllByRole("listitem");
    expect(items.length).toBe(3);
    // 原选中条目 e2 保持选中（索引从 1 变为 2）
    expect(items[2].classList.contains("active")).toBe(true);
    expect(mockedGetHistory.mock.calls.length).toBe(2);
  });

  it("未激活时收到 updated 不刷新", async () => {
    await renderPopup();
    handlers.get(CLIPBOARD_UPDATED_EVENT)?.({
      payload: { id: "e0" } as unknown as SessionPayload,
    });
    await flush();

    expect(mockedGetHistory).not.toHaveBeenCalled();
  });

  it("wheel 滚动切换选中（向下 +1，向上 -1，边界 clamp）", async () => {
    await renderPopup();
    fireShow(1);
    await flush();

    const card = document.querySelector(".qp-card")!;
    fireEvent.wheel(card, { deltaY: 100 });
    await flush();
    let items = screen.getAllByRole("listitem");
    expect(items[1].classList.contains("active")).toBe(true);

    fireEvent.wheel(card, { deltaY: -100 });
    await flush();
    items = screen.getAllByRole("listitem");
    expect(items[0].classList.contains("active")).toBe(true);

    // 顶部继续向上 → clamp 在 0
    fireEvent.wheel(card, { deltaY: -100 });
    await flush();
    expect(screen.getAllByRole("listitem")[0].classList.contains("active")).toBe(true);
  });

  it("release 后回写选中项并 close（携带会话 id）", async () => {
    await renderPopup();
    fireShow(1);
    await flush();

    fireEvent.wheel(document.querySelector(".qp-card")!, { deltaY: 100 }); // 选中第二项
    await flush();

    fireRelease(1);
    await flush();

    expect(mockedWrite).toHaveBeenCalledWith("e2");
    expect(mockedClose).toHaveBeenCalledWith(1);
  });

  it("过期会话的 release 被忽略", async () => {
    await renderPopup();
    fireShow(2); // 会话 2
    await flush();
    fireRelease(1); // 旧会话 1 的 release
    await flush();

    expect(mockedWrite).not.toHaveBeenCalled();
    expect(mockedClose).not.toHaveBeenCalled();
  });

  it("空历史时 release 只关闭不回写", async () => {
    mockedGetHistory.mockResolvedValue([]);
    await renderPopup();
    fireShow(1);
    await flush();

    fireRelease(1);
    await flush();

    expect(mockedWrite).not.toHaveBeenCalled();
    expect(mockedClose).toHaveBeenCalledWith(1);
  });

  it("Esc 取消：直接关闭不回写", async () => {
    await renderPopup();
    fireShow(1);
    await flush();

    fireEvent.keyDown(window, { key: "Escape" });
    await flush();

    expect(mockedWrite).not.toHaveBeenCalled();
    expect(mockedClose).toHaveBeenCalledWith(1);
  });

  it("F 键收藏/取消收藏选中条目并广播 updated 事件", async () => {
    await renderPopup();
    fireShow(1);
    await flush();

    // 初始未收藏 → F → setEntryFavorite(id, true) + emit（跨窗同步）
    fireEvent.keyDown(window, { key: "f" });
    await flush();
    expect(mockedSetFavorite).toHaveBeenCalledWith("e1", true);
    expect(mockedEmit).toHaveBeenCalledWith(CLIPBOARD_UPDATED_EVENT, { id: "e1" });
  });

  it("已收藏条目星标显示实心态，F 再按取消收藏", async () => {
    mockedGetHistory.mockResolvedValue([
      { ...TEXT_ENTRY, favoritedAt: "2026-08-13T02:00:00Z" },
      IMG_ENTRY,
    ]);
    await renderPopup();
    fireShow(1);
    await flush();

    const star = document.querySelector(".qp-star")!;
    expect(star.classList.contains("favorited")).toBe(true);
    expect(star.getAttribute("aria-pressed")).toBe("true");

    fireEvent.keyDown(window, { key: "F" });
    await flush();
    expect(mockedSetFavorite).toHaveBeenCalledWith("e1", false);
  });

  it("点击星标按钮切换收藏（不触发回写）", async () => {
    await renderPopup();
    fireShow(1);
    await flush();

    const star = document.querySelector(".qp-star")!;
    fireEvent.click(star);
    await flush();

    expect(mockedSetFavorite).toHaveBeenCalledWith("e1", true);
    expect(mockedEmit).toHaveBeenCalledWith(CLIPBOARD_UPDATED_EVENT, { id: "e1" });
    expect(mockedWrite).not.toHaveBeenCalled();
  });

  it("收藏变更刷新后保持选中条目", async () => {
    await renderPopup();
    fireShow(1);
    await flush();

    // 模拟：收藏 e1 后列表刷新为收藏区在前
    mockedGetHistory.mockResolvedValue([
      { ...TEXT_ENTRY, favoritedAt: "2026-08-13T02:00:00Z" },
      IMG_ENTRY,
    ]);
    handlers.get(CLIPBOARD_UPDATED_EVENT)?.({
      payload: { id: "e1" } as unknown as SessionPayload,
    });
    await flush();

    const items = screen.getAllByRole("listitem");
    expect(items[0].classList.contains("active")).toBe(true);
    expect(mockedGetHistory.mock.calls.length).toBe(2);
  });
});
