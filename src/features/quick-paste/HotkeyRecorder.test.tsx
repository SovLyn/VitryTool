import { fireEvent, render, screen, cleanup } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "../../i18n";
import {
  formatHotkeyForDisplay,
  HotkeyRecorder,
  keyToCode,
} from "./HotkeyRecorder";

afterEach(() => cleanup());

function renderRecorder(value = "", onChange = vi.fn()) {
  return render(() => (
    <I18nProvider>
      <HotkeyRecorder value={value} onChange={onChange} />
    </I18nProvider>
  ));
}

/** 向 window 派发 keydown（组件在录制态监听 window）。 */
function pressKey(init: KeyboardEventInit) {
  fireEvent.keyDown(window, init);
}

describe("HotkeyRecorder 快捷键录制组件", () => {
  it("展示当前快捷键的本地化格式，未设置显示占位", () => {
    renderRecorder("CommandOrControl+Shift+K");
    expect(screen.getByText("Ctrl + Shift + K")).toBeTruthy();
    expect(screen.getByText("清除快捷键")).toBeTruthy();
  });

  it("keyToCode 白名单映射", () => {
    expect(keyToCode("k")).toBe("K");
    expect(keyToCode("5")).toBe("5");
    expect(keyToCode("F12")).toBe("F12");
    expect(keyToCode(" ")).toBe("Space");
    expect(keyToCode("ArrowDown")).toBe("ArrowDown");
    expect(keyToCode("Control")).toBeNull(); // 纯修饰键
    expect(keyToCode("CapsLock")).toBeNull();
    expect(keyToCode("F13")).toBeNull(); // 超出白名单
  });

  it("formatHotkeyForDisplay 本地化展示", () => {
    expect(formatHotkeyForDisplay("CommandOrControl+Shift+K")).toBe("Ctrl + Shift + K");
    expect(formatHotkeyForDisplay("Alt+1")).toBe("Alt + 1");
    expect(formatHotkeyForDisplay("Super+ArrowDown")).toBe("Win + ↓");
  });

  it("录制 Ctrl+Shift+K → onChange 收到标准格式", async () => {
    const onChange = vi.fn();
    renderRecorder("", onChange);

    fireEvent.click(screen.getByText("未设置"));
    expect(screen.getByText("按下组合键…（Esc 取消）")).toBeTruthy();

    pressKey({ key: "k", ctrlKey: true, shiftKey: true });
    expect(onChange).toHaveBeenCalledWith("CommandOrControl+Shift+K");
    // 录制结束，回到展示态
    expect(screen.getByText("未设置")).toBeTruthy();
  });

  it("纯修饰键按下被忽略（继续等待主键）", () => {
    const onChange = vi.fn();
    renderRecorder("", onChange);
    fireEvent.click(screen.getByText("未设置"));

    pressKey({ key: "Control", ctrlKey: true });
    pressKey({ key: "Shift", shiftKey: true });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByText("按下组合键…（Esc 取消）")).toBeTruthy();
  });

  it("无修饰键 / 仅 Shift 组合被拒绝并提示", () => {
    const onChange = vi.fn();
    renderRecorder("", onChange);
    fireEvent.click(screen.getByText("未设置"));

    pressKey({ key: "k" });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByText("需包含至少一个 Ctrl / Alt / Win 修饰键")).toBeTruthy();

    pressKey({ key: "k", shiftKey: true });
    expect(onChange).not.toHaveBeenCalled();
  });

  it("Esc 取消录制", () => {
    const onChange = vi.fn();
    renderRecorder("", onChange);
    fireEvent.click(screen.getByText("未设置"));

    pressKey({ key: "Escape" });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByText("未设置")).toBeTruthy();
  });

  it("录制 Alt+1 与 Win+Space 组合", () => {
    const onChange = vi.fn();
    // 受控组件：模拟父组件在 onChange 后更新 value
    const Controlled = () => {
      const [value, setValue] = createSignal("");
      return (
        <I18nProvider>
          <HotkeyRecorder value={value()} onChange={(hk) => { setValue(hk); onChange(hk); }} />
        </I18nProvider>
      );
    };
    render(() => <Controlled />);
    fireEvent.click(screen.getByText("未设置"));

    pressKey({ key: "1", altKey: true });
    expect(onChange).toHaveBeenCalledWith("Alt+1");

    fireEvent.click(screen.getByText("Alt + 1")); // 再次进入录制（展示值为新快捷键）
    pressKey({ key: " ", metaKey: true });
    expect(onChange).toHaveBeenCalledWith("Super+Space");
  });

  it("清除按钮回调空串", () => {
    const onChange = vi.fn();
    renderRecorder("Alt+1", onChange);
    fireEvent.click(screen.getByText("清除快捷键"));
    expect(onChange).toHaveBeenCalledWith("");
  });
});
