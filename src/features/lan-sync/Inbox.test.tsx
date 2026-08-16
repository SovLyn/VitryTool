import { cleanup, fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "../../i18n";
import { Inbox } from "./Inbox";

const inboxResp = {
  nodes: [
    {
      peerId: "12D3KooTestNode1",
      terminalName: "SILVERBOX",
      entries: [
        {
          id: "e1",
          peerId: "12D3KooTestNode1",
          terminalName: "SILVERBOX",
          receivedAt: "2026-08-14T10:00:02Z",
          sentAt: "2026-08-14T10:00:01Z",
          text: "hello from silverbox",
          fingerprint: "f1",
        },
      ],
    },
    {
      peerId: "12D3KooTestNode2",
      terminalName: "",
      entries: [
        {
          id: "e2",
          peerId: "12D3KooTestNode2",
          terminalName: "",
          receivedAt: "2026-08-14T10:00:00Z",
          sentAt: "2026-08-14T10:00:00Z",
          text: "只发文本",
          fingerprint: "f2",
        },
      ],
    },
  ],
};

vi.mock("../../api/lan-sync", () => ({
  getLanInbox: vi.fn(async () => inboxResp),
  getLanSyncStatus: vi.fn(async () => ({
    peerId: "self",
    terminalName: "SOVLYN",
    broadcastEnabled: true,
    receiveEnabled: true,
    nodeRunning: true,
    peerCount: 2,
  })),
  writeLanInboxEntry: vi.fn(async () => undefined),
  deleteLanInboxEntry: vi.fn(async () => undefined),
  clearLanInbox: vi.fn(async () => undefined),
  LAN_INBOX_UPDATED_EVENT: "lan-sync://inbox-updated",
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));

// 通知系统（0.2.8）：页面操作反馈经全局通知；mock 掉 invoke 依赖，断言调用
vi.mock("../../api/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/notify")>();
  return { ...actual, notify: vi.fn(async () => undefined) };
});

import { getLanInbox, writeLanInboxEntry, deleteLanInboxEntry, clearLanInbox } from "../../api/lan-sync";
import { notify } from "../../api/notify";

afterEach(() => cleanup());
beforeEach(() => vi.clearAllMocks());

function renderInbox() {
  return render(() => (
    <I18nProvider>
      <Inbox onSeen={() => {}} />
    </I18nProvider>
  ));
}

describe("Inbox 收件箱页", () => {
  it("按节点分组渲染：终端名 + 条目预览 + 类型标记", async () => {
    renderInbox();
    expect(await screen.findByText("SILVERBOX")).toBeTruthy();
    expect(await screen.findByText("hello from silverbox")).toBeTruthy();
    expect(screen.getAllByText("文本").length).toBeGreaterThan(0);
  });

  it("无终端名时回退显示 peerId 短号", async () => {
    renderInbox();
    // 分组头与 meta 区展示 peerId 短号（含无终端名的节点）
    const shorts = await screen.findAllByText(/12D3Koo/);
    expect(shorts.length).toBeGreaterThanOrEqual(1);
  });

  it("单击条目触发回写，成功后发 success 通知（0.2.8 迁移）", async () => {
    renderInbox();
    const card = await screen.findByText("hello from silverbox");
    fireEvent.click(card);
    // 回写为 async：冲刷 microtask 后断言通知（契约 notify 5.6）
    await Promise.resolve();
    await Promise.resolve();
    expect(writeLanInboxEntry).toHaveBeenCalledWith("e1");
    expect(notify).toHaveBeenCalledWith({ level: "success", code: "lanSync.writtenBack" });
  });

  it("点击删除按钮触发单条删除（不触发回写）", async () => {
    renderInbox();
    const preview = await screen.findByText("hello from silverbox");
    const card = preview.closest(".entry-card")!;
    const deleteBtn = card.querySelector(".entry-delete")!;
    fireEvent.click(deleteBtn);
    expect(deleteLanInboxEntry).toHaveBeenCalledWith("e1");
    expect(writeLanInboxEntry).not.toHaveBeenCalled();
  });

  it("清空按钮弹出确认对话框，确认后触发 clearLanInbox（0.2.8 替代 window.confirm）", async () => {
    renderInbox();
    const clearBtn = await screen.findByText("清空全部");
    fireEvent.click(clearBtn);
    // 对话框出现（含确认与取消按钮）
    const dialog = screen.getByRole("alertdialog");
    expect(dialog).toBeTruthy();
    const confirmBtn = within(dialog).getByRole("button", { name: "清空全部" });
    fireEvent.click(confirmBtn);
    expect(clearLanInbox).toHaveBeenCalled();
  });

  it("清空确认对话框取消不触发 clearLanInbox", async () => {
    renderInbox();
    const clearBtn = await screen.findByText("清空全部");
    fireEvent.click(clearBtn);
    const dialog = screen.getByRole("alertdialog");
    const cancelBtn = within(dialog).getByRole("button", { name: "取消" });
    fireEvent.click(cancelBtn);
    expect(clearLanInbox).not.toHaveBeenCalled();
  });

  it("初始加载拉取一次收件箱", async () => {
    renderInbox();
    await screen.findByText("hello from silverbox");
    expect(getLanInbox).toHaveBeenCalled();
  });
});
