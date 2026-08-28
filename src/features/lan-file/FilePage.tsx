//! lan-file「文件」页（0.3.0，契约 `docs/api/lan-file.md`；设计决策 F1/F3）。
//!
//! 交互：拖放区 + 终端卡网格 + 传输卡。拖入文件 → 预填待发列表；
//! 点击终端卡 → 对该终端发起传输（多文件串行）；传输卡实时进度/速率/续传横幅。

import { listen } from "@tauri-apps/api/event";
import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import {
  getLanFilePeers,
  getLanFileStatus,
  LAN_FILE_PEERS_UPDATED_EVENT,
  LAN_FILE_TRANSFER_UPDATED_EVENT,
  sendLanFile,
  type LanFilePeer,
  type LanFileStatus,
  type LanFileTransfer,
} from "../../api/lan-file";
import { notify } from "../../api/notify";
import { getErrorCode } from "../../api/clipboard-history";
import { useI18n } from "../../i18n";
import { TransferCard } from "./TransferCard";

/** 待发文件行（拖入或选择后预填）。 */
interface PendingFile {
  path: string;
  name: string;
  size: number;
}

export function FilePage() {
  const { t } = useI18n();
  const [peers, setPeers] = createSignal<LanFilePeer[]>([]);
  const [status, setStatus] = createSignal<LanFileStatus | null>(null);
  const [pending, setPending] = createSignal<PendingFile[]>([]);
  const [transfer, setTransfer] = createSignal<LanFileTransfer | null>(null);
  const [dragOver, setDragOver] = createSignal(false);
  const [busyPeer, setBusyPeer] = createSignal<string | null>(null);

  onMount(() => {
    void refresh();
    setupNativeDrop();
    const unlistenPeers = listen(LAN_FILE_PEERS_UPDATED_EVENT, () => void refresh());
    const unlistenTransfer = listen<LanFileTransfer>(
      LAN_FILE_TRANSFER_UPDATED_EVENT,
      (e) => {
        setTransfer(e.payload ?? null);
      },
    );
    onCleanup(() => {
      void unlistenPeers.then((fn) => fn());
      void unlistenTransfer.then((fn) => fn());
    });
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
        addPaths(selected);
      } else if (typeof selected === "string") {
        addPaths([selected]);
      }
    } catch {
      await notify({ level: "warning", code: "lanFile.pickFailed" });
    }
  }

  /** 全局拖放监听（tauri 窗口级 onDragDrop 携带真实绝对路径）。 */
  function setupNativeDrop() {
    type DropPayload = { paths: string[]; position: { x: number; y: number } };
    const unlistenDrag = listen<{ type: string; payload: DropPayload }>("tauri://drag-drop", (e) => {
      if (e.payload?.type === "drop" || (e.payload as unknown as { paths?: string[] })?.paths) {
        const paths = (e.payload as unknown as { paths?: string[] }).paths;
        if (paths?.length) addPaths(paths);
      }
    });
    onCleanup(() => void unlistenDrag.then((fn) => fn()));
  }

  /** 预填待发路径列表（dialog 选择器 / 原生拖放回调；去重 + 上限 100，契约 5.6）。 */
  function addPaths(paths: string[]) {
    const next = [...pending()];
    for (const p of paths) {
      if (next.some((f) => f.path === p)) continue; // 去重
      const name = p.split(/[\\/]/).pop() || p;
      next.push({ path: p, name, size: 0 }); // size 由后端 sendLanFile 校验时确定
    }
    setPending(next.slice(0, 100)); // 契约 5.6：≤100 文件
  }

  function removePending(path: string) {
    setPending(pending().filter((f) => f.path !== path));
  }

  async function sendTo(peer: LanFilePeer) {
    const paths = pending().map((f) => f.path);
    if (paths.length === 0) {
      await notify({ level: "warning", code: "lanFile.pickFirst" });
      return;
    }
    if (!peer.supportsInteractive) {
      await notify({ level: "error", code: "lanFile.peer_unsupported" });
      return;
    }
    setBusyPeer(peer.peerId);
    try {
      await sendLanFile(peer.peerId, paths);
      // 发起成功：清空预填，传输卡接管展示
      setPending([]);
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lan_file.transfer_failed" });
    } finally {
      setBusyPeer(null);
    }
  }

  const enabled = () => status()?.enabled ?? false;
  const activeTransfer = () => {
    const tf = transfer();
    return tf && tf.transferId && (tf.state === "transferring" || tf.state === "offering" || tf.state === "resuming") ? tf : null;
  };

  return (
    <section class="file-page">
      {/* 总开关关闭提示 */}
      <Show when={!enabled()}>
        <div class="settings-row">
          <div class="message warning">
            {t("lanFile.disabledHint")}
          </div>
        </div>
      </Show>

      {/* 拖放区 / 预填列表 */}
      <div
        class={dragOver() ? "file-dropzone dragover" : "file-dropzone"}
        onDragOver={(e) => {
          e.preventDefault();
          setDragOver(true);
        }}
        onDragLeave={() => setDragOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragOver(false);
          // WebView DOM 拖放只有 File 对象（无完整路径）；真实路径由
          // tauri 全局 onDragDrop 事件（已监听，见 onMount）或 dialog 选择器提供
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

      {/* 选择文件按钮（tauri-plugin-dialog 原生多选，契约 5.6） */}
      <div class="file-pick-row">
        <button type="button" class="btn-ghost" onClick={() => void pickFiles()}>
          {t("lanFile.pickFiles")}
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
                  <Show when={peer.trusted} fallback={<span class="file-peer-untrusted">{t("lanFile.untrusted")}</span>}>
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

      {/* 传输卡（本会话内活跃或已完成摘要；无历史持久化，契约 5.3） */}
      <Show when={transfer()}>
        {(tf) => <TransferCard transfer={tf()} />}
      </Show>
    </section>
  );
}
