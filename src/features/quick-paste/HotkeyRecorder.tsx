//! 快捷键录制组件（纯前端，设置页使用）。
//!
//! 交互（契约 `docs/api/quick-paste.md` 第 5.1 节）：
//! - 点击进入录制态，`keydown` 捕获 `Ctrl / Alt / Shift / Super` + 主键组合；
//! - 纯修饰键按下忽略（等待主键）；`Esc` 取消录制；
//! - 必须包含至少一个非 Shift 修饰键（防止裸字母 / 仅 Shift 组合拦截常规输入）；
//! - 录制完成回调标准格式字符串（如 `CommandOrControl+Shift+K`）。

import { createEffect, createSignal, onCleanup, Show } from "solid-js";
import { useI18n } from "../../i18n";

export interface HotkeyRecorderProps {
  /** 当前快捷键（标准格式，空串 = 未设置）。 */
  value: string;
  /** 录制完成回调（标准格式）。 */
  onChange: (hotkey: string) => void;
}

/** 主键白名单映射（与后端 service 的解析白名单一致）：返回规范名，不支持返回 null。 */
export function keyToCode(key: string): string | null {
  if (/^[a-zA-Z]$/.test(key)) return key.toUpperCase();
  if (/^[0-9]$/.test(key)) return key;
  if (/^F([1-9]|1[0-2])$/i.test(key)) return key.toUpperCase();
  switch (key) {
    case " ":
      return "Space";
    case "Enter":
      return "Enter";
    case "Tab":
      return "Tab";
    case "Backspace":
      return "Backspace";
    case "Delete":
      return "Delete";
    case "Home":
      return "Home";
    case "End":
      return "End";
    case "PageUp":
      return "PageUp";
    case "PageDown":
      return "PageDown";
    case "Insert":
      return "Insert";
    case "ArrowUp":
      return "ArrowUp";
    case "ArrowDown":
      return "ArrowDown";
    case "ArrowLeft":
      return "ArrowLeft";
    case "ArrowRight":
      return "ArrowRight";
    default:
      return null;
  }
}

/** 标准格式 → 用户可读展示（"CommandOrControl+Shift+K" → "Ctrl + Shift + K"）。 */
export function formatHotkeyForDisplay(hotkey: string): string {
  return hotkey
    .split("+")
    .map((part) => {
      switch (part) {
        case "CommandOrControl":
          return "Ctrl";
        case "Super":
          return "Win";
        case "ArrowUp":
          return "↑";
        case "ArrowDown":
          return "↓";
        case "ArrowLeft":
          return "←";
        case "ArrowRight":
          return "→";
        case "PageUp":
          return "PgUp";
        case "PageDown":
          return "PgDn";
        default:
          return part;
      }
    })
    .join(" + ");
}

export function HotkeyRecorder(props: HotkeyRecorderProps) {
  const { t } = useI18n();
  const [recording, setRecording] = createSignal(false);
  const [error, setError] = createSignal("");

  /** keydown 处理：仅录制态生效；返回 true 表示已消费（阻止继续传播）。 */
  function onKeyDown(e: KeyboardEvent) {
    if (!recording()) return;
    e.preventDefault();
    e.stopPropagation();

    if (e.key === "Escape") {
      setRecording(false);
      setError("");
      return;
    }

    const code = keyToCode(e.key);
    if (!code) return; // 纯修饰键（Control/Shift/Alt/Meta）或其他不支持键：继续等待

    const mods: string[] = [];
    if (e.ctrlKey) mods.push("CommandOrControl");
    if (e.altKey) mods.push("Alt");
    if (e.shiftKey) mods.push("Shift");
    if (e.metaKey) mods.push("Super");

    // 至少一个非 Shift 修饰键（契约 5.1-③）
    if (mods.length === 0 || mods.every((m) => m === "Shift")) {
      setError(t("quickPaste.invalidNoModifier"));
      return;
    }

    const hotkey = [...mods, code].join("+");
    setRecording(false);
    setError("");
    props.onChange(hotkey);
  }

  // 录制态期间在 window 捕获键盘（焦点可在任意位置）
  createEffect(() => {
    if (!recording()) return;
    const handler = (e: KeyboardEvent) => onKeyDown(e);
    window.addEventListener("keydown", handler, true);
    onCleanup(() => window.removeEventListener("keydown", handler, true));
  });

  return (
    <div class="hotkey-recorder">
      <button
        type="button"
        class={recording() ? "hotkey-input recording" : "hotkey-input"}
        onClick={() => {
          setRecording(!recording());
          setError("");
        }}
        aria-pressed={recording()}
      >
        {recording() ? t("quickPaste.recordHint") : props.value ? formatHotkeyForDisplay(props.value) : t("quickPaste.notSet")}
      </button>
      <Show when={props.value && !recording()}>
        <button
          type="button"
          class="hotkey-clear"
          onClick={() => props.onChange("")}
          aria-label={t("quickPaste.clear")}
        >
          {t("quickPaste.clear")}
        </button>
      </Show>
      <Show when={error()}>
        <span class="settings-error">{error()}</span>
      </Show>
    </div>
  );
}
