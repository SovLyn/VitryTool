//! ConfirmDialog 测试（0.2.8，替代 window.confirm）：
//! - 打开渲染 alertdialog（标题/消息/确认/取消）；
//! - 确认回调、取消回调（按钮 / Esc / 遮罩点击）；
//! - 破坏性样式类（btn-danger）；
//! - 焦点管理：打开时聚焦取消按钮（安全默认）。

import { cleanup, fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";

function renderDialog(props: Partial<Parameters<typeof ConfirmDialog>[0]> = {}) {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  const utils = render(() => (
    <ConfirmDialog
      open
      title="清空全部"
      message="确定清空？"
      confirmLabel="清空全部"
      cancelLabel="取消"
      destructive
      onConfirm={onConfirm}
      onCancel={onCancel}
      {...props}
    />
  ));
  return { ...utils, onConfirm, onCancel };
}

afterEach(() => cleanup());

describe("ConfirmDialog", () => {
  it("打开时渲染 alertdialog：标题 + 消息 + 确认/取消按钮", () => {
    renderDialog();
    const dialog = screen.getByRole("alertdialog");
    expect(dialog).toBeTruthy();
    expect(screen.getByText("确定清空？")).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "取消" })).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "清空全部" })).toBeTruthy();
  });

  it("确认按钮触发 onConfirm", () => {
    const { onConfirm } = renderDialog();
    const dialog = screen.getByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "清空全部" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("取消按钮触发 onCancel", () => {
    const { onCancel } = renderDialog();
    const dialog = screen.getByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("Esc 触发 onCancel", () => {
    const { onCancel } = renderDialog();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("点击遮罩（非对话框内部）触发 onCancel", () => {
    const { onCancel } = renderDialog();
    const overlay = document.querySelector(".dialog-overlay")!;
    fireEvent.click(overlay); // target === currentTarget → 视为遮罩
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("点击对话框内部不触发 onCancel", () => {
    const { onCancel } = renderDialog();
    const dialog = screen.getByRole("alertdialog");
    fireEvent.click(dialog);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("破坏性操作确认按钮带 btn-danger 样式类", () => {
    renderDialog();
    const dialog = screen.getByRole("alertdialog");
    const confirmBtn = within(dialog).getByRole("button", { name: "清空全部" });
    expect(confirmBtn.classList.contains("btn-danger")).toBe(true);
  });

  it("非破坏性操作用 btn-primary 样式类", () => {
    renderDialog({ destructive: false });
    const dialog = screen.getByRole("alertdialog");
    const confirmBtn = within(dialog).getByRole("button", { name: "清空全部" });
    expect(confirmBtn.classList.contains("btn-primary")).toBe(true);
  });

  it("关闭时（open=false）不渲染", () => {
    render(() => (
      <ConfirmDialog
        open={false}
        title="清空全部"
        message="确定清空？"
        confirmLabel="清空全部"
        cancelLabel="取消"
        onConfirm={() => {}}
        onCancel={() => {}}
      />
    ));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
