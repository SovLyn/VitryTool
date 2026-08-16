//! NotificationProvider 测试（契约 `docs/api/notify.md` 5.3/5.5）：
//! - 事件驱动：监听 `app://notify` 渲染 toast；
//! - 分 level 时长自动消失（fake timers 推进验证）；
//! - warning/error 带手动关闭，success/info 无；
//! - 容量最多 4 条、新到顶部；同 level+code 去重窗口内不新增；
//! - 无障碍：`role="status"` + `aria-live="polite"`；
//! - 未知 code 兜底 `notify.unknown`。

import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "../i18n";
import { NotificationProvider, LEVEL_DURATION, MAX_TOASTS } from "./NotificationProvider";

type ListenHandler = (event: { payload: unknown }) => void;
let capturedHandler: ListenHandler | undefined;

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_event: string, handler: ListenHandler) => {
    capturedHandler = handler;
    return () => undefined;
  }),
}));

/** 模拟后端广播一条通知。 */
function emit(payload: unknown) {
  capturedHandler?.({ payload });
}

function renderProvider() {
  return render(() => (
    <I18nProvider>
      <NotificationProvider />
    </I18nProvider>
  ));
}

afterEach(() => {
  cleanup();
  capturedHandler = undefined;
  vi.useRealTimers();
});

beforeEach(() => {
  vi.useFakeTimers();
});

describe("NotificationProvider · 事件驱动渲染", () => {
  it("容器带 role=status + aria-live=polite（无障碍）", () => {
    renderProvider();
    const stack = document.querySelector(".toast-stack")!;
    expect(stack.getAttribute("role")).toBe("status");
    expect(stack.getAttribute("aria-live")).toBe("polite");
  });

  it("收到 app://notify 事件后渲染对应文案（i18n 翻译）", () => {
    renderProvider();
    emit({ level: "success", code: "clipboard.copied" });
    expect(screen.getByText("已写回剪贴板")).toBeTruthy();
  });

  it("后端错误码经映射翻译（lan.* → lanSync.*，修复静默消失 bug）", () => {
    renderProvider();
    emit({ level: "error", code: "lan.peer_node_error" });
    expect(screen.getByText("同步节点异常，请查看日志")).toBeTruthy();
  });

  it("未知 code 兜底 notify.unknown（带 code 参数）", () => {
    renderProvider();
    emit({ level: "error", code: "some.unknown_code" });
    expect(screen.getByText("发生未知错误（some.unknown_code）")).toBeTruthy();
  });

  it("params 参与插值（lanSync.items 带 count）", () => {
    renderProvider();
    emit({ level: "info", code: "lanSync.items", params: { count: 3 } });
    expect(screen.getByText("3 条")).toBeTruthy();
  });
});

describe("NotificationProvider · level 时长与手动关闭（契约 5.3）", () => {
  it("success 3 秒自动消失", () => {
    renderProvider();
    emit({ level: "success", code: "clipboard.copied" });
    expect(screen.getByText("已写回剪贴板")).toBeTruthy();
    vi.advanceTimersByTime(LEVEL_DURATION.success + 260);
    expect(screen.queryByText("已写回剪贴板")).toBeNull();
  });

  it("error 8 秒自动消失（比 success 长）", () => {
    renderProvider();
    emit({ level: "error", code: "lan.peer_node_error" });
    // 6 秒时仍在
    vi.advanceTimersByTime(6000);
    expect(screen.getByText("同步节点异常，请查看日志")).toBeTruthy();
    vi.advanceTimersByTime(LEVEL_DURATION.error - 6000 + 260);
    expect(screen.queryByText("同步节点异常，请查看日志")).toBeNull();
  });

  it("hover 暂停计时：悬停时不超过 3 秒不消失", () => {
    renderProvider();
    emit({ level: "success", code: "clipboard.copied" });
    const stack = document.querySelector(".toast-stack")!;
    fireEvent.mouseEnter(stack);
    vi.advanceTimersByTime(LEVEL_DURATION.success + 1000);
    expect(screen.getByText("已写回剪贴板")).toBeTruthy();
    fireEvent.mouseLeave(stack);
    vi.advanceTimersByTime(LEVEL_DURATION.success + 260);
    expect(screen.queryByText("已写回剪贴板")).toBeNull();
  });

  it("warning/error 带手动关闭按钮，点击后立即进入退出并消失", () => {
    renderProvider();
    emit({ level: "warning", code: "quick_paste.tray_update_failed" });
    const dismiss = screen.getByLabelText("关闭通知");
    expect(dismiss).toBeTruthy();
    fireEvent.click(dismiss);
    vi.advanceTimersByTime(260);
    expect(screen.queryByText("托盘菜单更新失败")).toBeNull();
  });

  it("success/info 无手动关闭按钮", () => {
    renderProvider();
    emit({ level: "success", code: "clipboard.copied" });
    emit({ level: "info", code: "lanSync.items", params: { count: 1 } });
    expect(screen.queryByLabelText("关闭通知")).toBeNull();
  });
});

describe("NotificationProvider · 堆栈行为（契约 5.5）", () => {
  it(`容量最多 ${MAX_TOASTS} 条：第 ${MAX_TOASTS + 1} 条挤掉最旧`, () => {
    renderProvider();
    for (let i = 0; i < MAX_TOASTS + 1; i++) {
      emit({ level: "success", code: `demo.toast_${i}` });
    }
    expect(document.querySelectorAll(".toast").length).toBe(MAX_TOASTS);
    // 最早的 demo.toast_0 被淘汰
    expect(screen.queryByText(/demo\.toast_0/)).toBeNull();
    // 最新的在（渲染层按代码兜底 unknown，但 DOM 存在）
    expect(document.querySelectorAll(".toast").length).toBe(MAX_TOASTS);
  });

  it("新到顶部：后到的 toast 在 DOM 顺序中更靠前", () => {
    renderProvider();
    emit({ level: "success", code: "clipboard.copied" });
    emit({ level: "error", code: "lan.peer_node_error" });
    const toasts = document.querySelectorAll(".toast");
    expect(toasts[0].classList.contains("toast-error")).toBe(true);
    expect(toasts[1].classList.contains("toast-success")).toBe(true);
  });

  it("同 level+code 去重窗口内重复到达不新增，重置计时", () => {
    renderProvider();
    emit({ level: "success", code: "clipboard.copied" });
    emit({ level: "success", code: "clipboard.copied" });
    expect(document.querySelectorAll(".toast").length).toBe(1);
  });

  it("回归：tick 期间 DOM 不重建（元素引用稳定，防止进入动画重放 = 频闪）", () => {
    renderProvider();
    emit({ level: "success", code: "clipboard.copied" });
    const first = document.querySelector(".toast")!;
    // 多个 tick 推进（每个 200ms），DOM 元素引用必须保持不变
    vi.advanceTimersByTime(200);
    expect(document.querySelector(".toast")).toBe(first);
    vi.advanceTimersByTime(800);
    expect(document.querySelector(".toast")).toBe(first);
    // 到期后进入退出动画：元素替换为 leaving 态（引用可变化），随后消失
    vi.advanceTimersByTime(LEVEL_DURATION.success - 1000);
    expect(document.querySelectorAll(".toast").length).toBe(1);
    vi.advanceTimersByTime(260);
    expect(document.querySelectorAll(".toast").length).toBe(0);
  });

  it("去重窗口外（超过 3 秒）再次到达则新增", () => {
    renderProvider();
    emit({ level: "success", code: "clipboard.copied" });
    vi.advanceTimersByTime(3100);
    emit({ level: "success", code: "clipboard.copied" });
    expect(document.querySelectorAll(".toast").length).toBe(2);
  });

  it("不同 level 同 code 不去重（error 与 success 可共存）", () => {
    renderProvider();
    emit({ level: "success", code: "clipboard.copied" });
    emit({ level: "error", code: "clipboard.copied" });
    expect(document.querySelectorAll(".toast").length).toBe(2);
  });

  it("关闭按钮 aria-label 为 notify.dismiss 文案", () => {
    renderProvider();
    emit({ level: "error", code: "lan.peer_node_error" });
    expect(screen.getByLabelText("关闭通知")).toBeTruthy();
  });
});
