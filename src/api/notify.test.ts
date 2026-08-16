//! 通知 api 封装测试（契约 `docs/api/notify.md`）：
//! - `resolveNotifyCode`：i18n 键直通 / 后端错误码映射（lan.*、quick_paste.*）/ 兜底；
//! - `notify()`：fire-and-forget 提交（成功静默、失败仅记日志不抛）。

import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { notify, resolveNotifyCode } from "./notify";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => undefined),
}));

const mockedInvoke = vi.mocked(invoke);

describe("resolveNotifyCode 错误码解析（契约 5.4）", () => {
  beforeEach(() => vi.clearAllMocks());

  it("i18n 键直接命中（clipboard.* 前后端一致，无需映射）", () => {
    expect(resolveNotifyCode("clipboard.copied")).toBe("clipboard.copied");
    expect(resolveNotifyCode("quickPaste.saved")).toBe("quickPaste.saved");
    expect(resolveNotifyCode("lanSync.writtenBack")).toBe("lanSync.writtenBack");
  });

  it("后端错误码 quick_paste.* → i18n 键 quickPaste.*", () => {
    expect(resolveNotifyCode("quick_paste.register_failed")).toBe(
      "quickPaste.register_failed",
    );
  });

  it("quick_paste.tray_update_failed 键名也映射（trayUpdateFailed 驼峰）", () => {
    expect(resolveNotifyCode("quick_paste.tray_update_failed")).toBe(
      "quickPaste.trayUpdateFailed",
    );
  });

  it("后端错误码 lan.* → i18n 键 lanSync.*（修复静默消失 bug）", () => {
    expect(resolveNotifyCode("lan.peer_node_error")).toBe("lanSync.peer_node_error");
    expect(resolveNotifyCode("lan.storage_error")).toBe("lanSync.storage_error");
    expect(resolveNotifyCode("lan.entry_not_found")).toBe("lanSync.entry_not_found");
  });

  it("未知 code 原样返回（渲染层再兜底 notify.unknown）", () => {
    expect(resolveNotifyCode("some.unknown_code")).toBe("some.unknown_code");
  });

  it("空 code 返回空串", () => {
    expect(resolveNotifyCode("")).toBe("");
  });
});

describe("notify() fire-and-forget 提交", () => {
  beforeEach(() => vi.clearAllMocks());

  it("以 payload 调用 invoke('notify')", async () => {
    const payload = { level: "error" as const, code: "lan.peer_node_error", params: {} };
    await notify(payload);
    expect(mockedInvoke).toHaveBeenCalledWith("notify", payload);
  });

  it("无 params 时不传", async () => {
    await notify({ level: "success", code: "clipboard.copied" });
    expect(mockedInvoke).toHaveBeenCalledWith("notify", {
      level: "success",
      code: "clipboard.copied",
    });
  });

  it("invoke 失败不抛出（fire-and-forget，仅记日志）", async () => {
    mockedInvoke.mockRejectedValueOnce(new Error("backend down"));
    await expect(
      notify({ level: "error", code: "notify.invalid" }),
    ).resolves.toBeUndefined();
  });
});
