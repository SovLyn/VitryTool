//! lan-file「文件」页（0.3.0，契约 `docs/api/lan-file.md`；设计决策 F1/F3）。
//!
//! 交互：拖放区 + 终端卡网格 + 传输卡。拖入文件 → 预填待发列表（App 层全局捕获
//! `onDragDropEvent` 并跳转到本页）；点击终端卡 → 对该终端发起传输（多文件串行）；
//! 传输卡实时进度/速率/续传横幅/取消/重试。
//!
//! 已选列表**发起后保留**（契约 F3：用户可再发给另一台终端；传输进度由卡片承担），
//! 失败时用同一批文件重新发起（重试）。

import { listen } from "@tauri-apps/api/event";
import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import {
  cancelLanFileTransfer,
  checkLanFilePaths,
  formatBytes,
  getLanFilePeers,
  getLanFileStatus,
  LAN_FILE_PEERS_UPDATED_EVENT,
  openReceiveFolder,
  revealSavedPath,
  sendLanFile,
  type LanFilePeer,
  type LanFileStatus,
} from "../../api/lan-file";
import { notify } from "../../api/notify";
import { getErrorCode } from "../../api/clipboard-history";
import { useI18n } from "../../i18n";
import { clearTransfer, lastSend, rememberSend, transfer } from "./transfer-store";
import { isTerminalState, TransferCard } from "./TransferCard";

/** 待发文件行（拖入或选择后预填）。 */
interface PendingFile {
  path: string;
  name: string;
  /** 字节数（0 = 未知）。 */
  size: number;
}

export interface FilePageProps {
  /** App 层全局拖入的路径（跳转本页后预填）。 */
  droppedPaths?: string[];
  /** 已消费拖入路径（清空 App 层缓冲，避免重复预填）。 */
  onDroppedConsumed?: () => void;
}

export function FilePage(props: FilePageProps) {
  const { t } = useI18n();
  const [peers, setPeers] = createSignal<LanFilePeer[]>([]);
  const [status, setStatus] = createSignal<LanFileStatus | null>(null);
  const [pending, setPending] = createSignal<PendingFile[]>([]);
  const [dragOver, setDragOver] = createSignal(false);
  const [busyPeer, setBusyPeer] = createSignal<string | null>(null);

  onMount(() => {
    void refresh();
    const unlistenPeers = listen(LAN_FILE_PEERS_UPDATED_EVENT, () => void refresh());
    onCleanup(() => {
      void unlistenPeers.then((fn) => fn());
    });
  });

  // App 层拖入预填（F1：全局拖入跳页预填）
  createEffect(() => {
    const dropped = props.droppedPaths;
    if (dropped && dropped.length > 0) {
      void addPaths(dropped);
      props.onDroppedConsumed?.();
    }
  });

  async function refresh() {
    try {
      const [p, s] = await Promise.all([getLanFilePeers(), getLanFileStatus()]);
      setPeers(p.peers ?? []);
      setStatus(s);
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lan_file.node_not_running" });
    }
  }

  /** 原生文件选择器（多选，契约 5.6：前端经 tauri-plugin-dialog 得到真实路径列表）。 */
  async function pickFiles() {
    try {
      const selected = await open({ multiple: true, directory: false });
      if (Array.isArray(selected)) {
        void addPaths(selected);
      } else if (typeof selected === "string") {
        void addPaths([selected]);
      }
    } catch {
      await notify({ level: "warning", code: "lanFile.pickFailed" });
    }
  }

  /**
   * 预填待发路径列表（dialog 选择器 / 全局拖放；去重 + 上限 100，契约 5.6）。
   *
   * 先经后端预检：**目录被忽略并提示**（v1 不支持文件夹传输），不可读文件同样跳过；
   * 顺带拿到文件大小用于展示。
   */
  async function addPaths(paths: string[]) {
    if (paths.length === 0) return;
    let folders = 0;
    let unreadable = 0;
    let items: { path: string; name: string; size: number }[] = [];
    try {
      const resp = await checkLanFilePaths(paths);
      for (const info of resp.paths) {
        if (info.isDir) {
          folders += 1;
        } else if (!info.readable) {
          unreadable += 1;
        } else {
          items.push({ path: info.path, name: info.name, size: info.size });
        }
      }
    } catch {
      // 预检不可用（后端未就绪）：退化为原样加入，由 sendLanFile 的命令层校验兜底报错
      items = paths.map((p) => ({ path: p, name: p.split(/[\\/]/).pop() || p, size: 0 }));
    }
    if (folders > 0) {
      await notify({
        level: "warning",
        code: "lanFile.foldersIgnored",
        params: { count: folders },
      });
    }
    if (unreadable > 0) {
      await notify({
        level: "warning",
        code: "lanFile.unreadableIgnored",
        params: { count: unreadable },
      });
    }
    if (items.length === 0) return;
    // 函数式更新：不在响应式作用域内读取 pending()（避免拖入预填的 effect 自触发循环）
    setPending((prev) => {
      const next = [...prev];
      for (const item of items) {
        if (next.some((f) => f.path === item.path)) continue; // 去重
        next.push(item);
      }
      return next.slice(0, 100); // 契约 5.6：≤100 文件
    });
  }

  function removePending(path: string) {
    setPending(pending().filter((f) => f.path !== path));
  }

  async function sendPaths(peerId: string, paths: string[], terminalName: string) {
    setBusyPeer(peerId);
    try {
      await sendLanFile(peerId, paths);
      rememberSend(peerId, paths);
      await notify({
        level: "info",
        code: "lanFile.sentRequest",
        params: { name: terminalName, count: paths.length },
      });
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lan_file.transfer_failed" });
    } finally {
      setBusyPeer(null);
    }
  }

  async function sendTo(peer: LanFilePeer) {
    const paths = pending().map((f) => f.path);
    if (paths.length === 0) {
      await notify({ level: "warning", code: "lanFile.pickFirst" });
      return;
    }
    if (!peer.supportsInteractive) {
      await notify({ level: "error", code: "lan_file.peer_unsupported" });
      return;
    }
    // 已选列表保留（用户可继续发给其他终端；进度由传输卡承担）
    await sendPaths(peer.peerId, paths, peer.terminalName);
  }

  /**
   * 失败/取消后把该次传输的文件**加回待传列表**（不自动重发）。
   *
   * 取代旧「重试」按钮：重试会把「选终端」这一步也替用户做了，而失败往往
   * 就是链路/对端的问题；加回列表后用户可自行换终端或等链路恢复再发。
   */
  async function addBackToPending() {
    const last = lastSend();
    if (!last || last.paths.length === 0) {
      await notify({ level: "warning", code: "lanFile.addBackUnavailable" });
      return;
    }
    await addPaths(last.paths);
    await notify({
      level: "success",
      code: "lanFile.addedToPending",
      params: { count: last.paths.length },
    });
  }

  async function cancelTransfer(transferId: string) {
    if (!transferId) return;
    try {
      await cancelLanFileTransfer(transferId);
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lan_file.transfer_failed" });
    }
  }

  async function reveal(path: string) {
    try {
      await revealSavedPath(path);
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lan_file.transfer_failed" });
    }
  }

  /** 一键打开本机接收文件夹（文件管理器）。 */
  async function openFolder() {
    try {
      await openReceiveFolder();
    } catch (err) {
      console.warn("lan-file: open receive folder failed:", err);
      await notify({ level: "error", code: "lanFile.openFolderFailed" });
    }
  }

  const enabled = () => status()?.enabled ?? false;
  const activeTransfer = () => {
    const tf = transfer();
    return tf && tf.transferId && !isTerminalState(tf.state) ? tf : null;
  };

  return (
    <section class="file-page">
      {/* 总开关关闭提示 */}
      <Show when={!enabled()}>
        <div class="settings-row">
          <div class="message warning">{t("lanFile.disabledHint")}</div>
        </div>
      </Show>

      {/* 拖放区 / 预填列表 */}
      <div
        class={["file-dropzone", pending().length > 0 ? "has-files" : "", dragOver() ? "dragover" : ""]
          .filter(Boolean)
          .join(" ")}
        onDragOver={(e) => {
          e.preventDefault();
          setDragOver(true);
        }}
        onDragLeave={() => setDragOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragOver(false);
          // WebView DOM 拖放只有 File 对象（无完整路径）；真实路径由 App 层
          // 全局 onDragDropEvent 提供（已监听），此处仅提示
          const files = Array.from(e.dataTransfer?.files ?? []);
          if (files.length > 0) {
            void notify({ level: "info", code: "lanFile.dropWebHint" });
          }
        }}
      >
        <Show
          when={pending().length > 0}
          fallback={<span class="file-dropzone-hint">{t("lanFile.dropHint")}</span>}
        >
          <ul class="file-pending-list">
            <For each={pending()}>
              {(f) => (
                <li class="file-pending-item">
                  <span class="file-pending-name" title={f.path}>
                    {f.name}
                  </span>
                  <Show when={f.size > 0}>
                    <span class="file-pending-size">{formatBytes(f.size)}</span>
                  </Show>
                  <button
                    type="button"
                    class="file-pending-remove"
                    onClick={() => removePending(f.path)}
                  >
                    {t("lanFile.remove")}
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>

      {/* 选择文件按钮（tauri-plugin-dialog 原生多选，契约 5.6）+ 打开接收文件夹 */}
      <div class="file-pick-row">
        <button type="button" class="btn-ghost" onClick={() => void pickFiles()}>
          {t("lanFile.pickFiles")}
        </button>
        <button
          type="button"
          class="btn-ghost"
          title={t("lanFile.openReceiveFolderHint")}
          onClick={() => void openFolder()}
        >
          {t("lanFile.openReceiveFolder")}
        </button>
        <Show when={pending().length > 0}>
          <span class="file-pick-count">
            {t("lanFile.pendingCount", { count: pending().length })}
          </span>
        </Show>
      </div>

      {/* 终端卡网格 */}
      <div class="file-peers-grid">
        <Show
          when={peers().filter((p) => p.supportsInteractive).length > 0}
          fallback={<div class="empty">{t("lanFile.noPeers")}</div>}
        >
          <For each={peers().filter((p) => p.supportsInteractive)}>
            {(peer) => (
              <button
                type="button"
                class="file-peer-card"
                disabled={busyPeer() !== null || !!activeTransfer()}
                title={peer.fingerprint}
                onClick={() => void sendTo(peer)}
              >
                <span class="file-peer-name">{peer.terminalName}</span>
                <span class="file-peer-meta">
                  <Show
                    when={peer.trusted}
                    fallback={<span class="file-peer-untrusted">{t("lanFile.untrusted")}</span>}
                  >
                    <span class="file-peer-trusted">{t("lanFile.trusted")}</span>
                  </Show>
                  <span class="file-peer-id">{peer.peerId.slice(0, 10)}…</span>
                </span>
                <span class="file-peer-action">{t("lanFile.sendTo")}</span>
              </button>
            )}
          </For>
        </Show>
      </div>

      {/* 传输卡（本会话内活跃或终态停留；无历史持久化，契约 5.3） */}
      <Show when={transfer()}>
        {(tf) => (
          <TransferCard
            transfer={tf()}
            onCancel={(id) => void cancelTransfer(id)}
            onAddToPending={() => void addBackToPending()}
            onDismiss={() => clearTransfer()}
            onReveal={(p) => void reveal(p)}
          />
        )}
      </Show>
    </section>
  );
}
