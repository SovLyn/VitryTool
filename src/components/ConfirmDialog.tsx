//! 通用确认对话框（0.2.8，替代 `window.confirm`——WebView 原生确认框会显示
//! "localhost:1420 显示" 这类宿主标题，观感割裂且不可控）。
//!
//! 设计（沿用项目 Apple 玻璃视觉，契约习惯同通知系统）：
//! - 模态：遮罩（点击取消）+ 居中玻璃卡（`--surface-raised` + backdrop-filter + `--shadow-lg`）；
//! - 破坏性操作（清空类）：确认按钮红色（`.btn-danger`），符合 macOS alert 惯例
//!   （按钮文案 = 具体动作，如「清空全部」，而非通用「确认」）；
//! - 无障碍：`role="alertdialog"` + `aria-modal` + labelledby/describedby；
//!   打开时焦点落到**取消**按钮（安全默认：回车不误触破坏性操作），关闭后还原焦点；
//!   Esc 或点击遮罩 = 取消；
//! - 动效：进入材质化（scale + translateY + opacity），`prefers-reduced-motion` 降级。

import { createEffect, createSignal, onCleanup, Show } from "solid-js";

interface ConfirmDialogProps {
  open: boolean;
  /** 标题（用具体操作名，如「清空全部」）。 */
  title: string;
  /** 描述文案（如「确定清空全部剪贴历史？」）。 */
  message: string;
  /** 确认按钮文案（= 具体动作）。 */
  confirmLabel: string;
  /** 取消按钮文案。 */
  cancelLabel: string;
  /** 破坏性操作（清空类）：确认按钮红色强调。 */
  destructive?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog(props: ConfirmDialogProps) {
  const [cancelBtn, setCancelBtn] = createSignal<HTMLButtonElement | null>(null);
  let restoreFocus: HTMLElement | null = null;

  // 焦点管理：打开时聚焦取消（安全默认），关闭时还原到触发按钮
  createEffect(() => {
    if (props.open) {
      restoreFocus = document.activeElement as HTMLElement | null;
      requestAnimationFrame(() => cancelBtn()?.focus());
    } else if (restoreFocus) {
      restoreFocus.focus();
      restoreFocus = null;
    }
  });

  // Esc 取消
  createEffect(() => {
    if (!props.open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") props.onCancel();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <Show when={props.open}>
      <div
        class="dialog-overlay"
        onClick={(e) => {
          if (e.target === e.currentTarget) props.onCancel();
        }}
      >
        <div
          class="dialog"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="confirm-dialog-title"
          aria-describedby="confirm-dialog-message"
        >
          <h2 id="confirm-dialog-title" class="dialog-title">
            {props.title}
          </h2>
          <p id="confirm-dialog-message" class="dialog-message">
            {props.message}
          </p>
          <div class="dialog-actions">
            <button type="button" class="btn-ghost" ref={setCancelBtn} onClick={props.onCancel}>
              {props.cancelLabel}
            </button>
            <button
              type="button"
              class={props.destructive ? "btn-danger" : "btn-primary"}
              onClick={props.onConfirm}
            >
              {props.confirmLabel}
            </button>
          </div>
        </div>
      </div>
    </Show>
  );
}
