import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  clearLanInbox,
  deleteLanInboxEntry,
  getLanInbox,
  getLanSyncStatus,
  LAN_INBOX_UPDATED_EVENT,
  setLanSyncBroadcast,
  setLanSyncReceive,
  setLanSyncTerminalName,
  writeLanInboxEntry,
} from "./lan-sync";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockedInvoke = vi.mocked(invoke);

describe("lan-sync api 封装（契约 docs/api/lan-sync.md）", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("getLanSyncStatus 调用 get_lan_sync_status", async () => {
    mockedInvoke.mockResolvedValue({
      peerId: "12D3KooA",
      terminalName: "SOVLYN",
      broadcastEnabled: true,
      receiveEnabled: true,
      nodeRunning: true,
      peerCount: 1,
    });
    const s = await getLanSyncStatus();
    expect(mockedInvoke).toHaveBeenCalledWith("get_lan_sync_status");
    expect(s.peerId).toBe("12D3KooA");
  });

  it("开关命令传递 enabled 参数", async () => {
    mockedInvoke.mockResolvedValue(undefined);
    await setLanSyncBroadcast(false);
    expect(mockedInvoke).toHaveBeenCalledWith("set_lan_sync_broadcast", { enabled: false });
    await setLanSyncReceive(true);
    expect(mockedInvoke).toHaveBeenCalledWith("set_lan_sync_receive", { enabled: true });
  });

  it("终端名命令传递 name 参数", async () => {
    mockedInvoke.mockResolvedValue(undefined);
    await setLanSyncTerminalName("MY-PC");
    expect(mockedInvoke).toHaveBeenCalledWith("set_lan_sync_terminal_name", { name: "MY-PC" });
  });

  it("收件箱命令传递 id / 无参数", async () => {
    mockedInvoke.mockResolvedValue(undefined);
    await writeLanInboxEntry("id-1");
    expect(mockedInvoke).toHaveBeenCalledWith("write_lan_inbox_entry", { id: "id-1" });
    await deleteLanInboxEntry("id-2");
    expect(mockedInvoke).toHaveBeenCalledWith("delete_lan_inbox_entry", { id: "id-2" });
    await clearLanInbox();
    expect(mockedInvoke).toHaveBeenCalledWith("clear_lan_inbox");
  });

  it("getLanInbox 返回分组结构", async () => {
    mockedInvoke.mockResolvedValue({
      nodes: [
        {
          peerId: "p1",
          terminalName: "A",
          entries: [{ id: "e1", peerId: "p1", terminalName: "A", receivedAt: "x", sentAt: "y", fingerprint: "f" }],
        },
      ],
    });
    const resp = await getLanInbox();
    expect(mockedInvoke).toHaveBeenCalledWith("get_lan_inbox");
    expect(resp.nodes[0].entries[0].id).toBe("e1");
  });

  it("事件名与契约一致", () => {
    expect(LAN_INBOX_UPDATED_EVENT).toBe("lan-sync://inbox-updated");
  });
});
