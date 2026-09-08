//! lan-file api 封装测试（契约 docs/api/lan-file.md）。

import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  checkLanFilePaths,
  formatBytes,
  formatSpeed,
  getLanFilePeers,
  getLanFileStatus,
  openReceiveFolder,
  sendLanFile,
} from "./lan-file";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => undefined),
  revealItemInDir: vi.fn(async () => undefined),
}));
vi.mock("@tauri-apps/api/path", () => ({
  appDataDir: vi.fn(async () => "C:\\Users\\x\\AppData\\Roaming\\com.sovly.vitrytool"),
  join: vi.fn(async (...parts: string[]) => parts.join("\\")),
}));

const mockedInvoke = vi.mocked(invoke);
const mockedOpenPath = vi.mocked(openPath);

describe("lan-file api 封装", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("getLanFileStatus 调用 get_lan_file_status", async () => {
    mockedInvoke.mockResolvedValueOnce({
      enabled: true,
      listening: true,
      tcpPort: 52341,
      peerCount: 2,
      trustedCount: 1,
    });
    const s = await getLanFileStatus();
    expect(mockedInvoke).toHaveBeenCalledWith("get_lan_file_status");
    expect(s.tcpPort).toBe(52341);
    expect(s.peerCount).toBe(2);
  });

  it("getLanFilePeers 调用 get_lan_file_peers", async () => {
    mockedInvoke.mockResolvedValueOnce({
      peers: [
        {
          peerId: "12D3Koo",
          terminalName: "SILVERBOX",
          fingerprint: "SHA256:abc",
          trusted: true,
          supportsInteractive: true,
        },
      ],
    });
    const r = await getLanFilePeers();
    expect(mockedInvoke).toHaveBeenCalledWith("get_lan_file_peers");
    expect(r.peers[0].trusted).toBe(true);
  });

  it("sendLanFile 传递 peerId 与 filePaths", async () => {
    mockedInvoke.mockResolvedValueOnce({ transferId: "t-1" });
    const r = await sendLanFile("peerA", ["C:\\a.txt", "C:\\b.bin"]);
    expect(mockedInvoke).toHaveBeenCalledWith("send_lan_file", {
      peerId: "peerA",
      filePaths: ["C:\\a.txt", "C:\\b.bin"],
    });
    expect(r.transferId).toBe("t-1");
  });

  it("checkLanFilePaths 传递路径列表并返回预检结果", async () => {
    mockedInvoke.mockResolvedValueOnce({
      paths: [{ path: "C:\\a.txt", name: "a.txt", size: 5, isDir: false, readable: true }],
    });
    const r = await checkLanFilePaths(["C:\\a.txt"]);
    expect(mockedInvoke).toHaveBeenCalledWith("check_lan_file_paths", {
      filePaths: ["C:\\a.txt"],
    });
    expect(r.paths[0].readable).toBe(true);
  });

  it("openReceiveFolder 打开接收目录（AppData/lanfile）", async () => {
    mockedOpenPath.mockClear();
    await openReceiveFolder();
    expect(mockedOpenPath).toHaveBeenCalledWith(
      "C:\\Users\\x\\AppData\\Roaming\\com.sovly.vitrytool\\lanfile",
    );
  });
});

describe("formatBytes / formatSpeed", () => {
  it("字节人性化展示", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1024 * 1024)).toBe("1.0 MB");
    expect(formatBytes(1536 * 1024 * 1024)).toBe("1.5 GB");
  });

  it("速率展示带 /s", () => {
    expect(formatSpeed(0)).toBe("");
    expect(formatSpeed(1024 * 100)).toBe("100.0 KB/s");
  });
});
