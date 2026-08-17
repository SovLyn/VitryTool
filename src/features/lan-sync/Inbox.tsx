//! 收件箱页：按来源节点分组的列表（lan-sync，契约 docs/api/lan-sync.md）。
//!
//! 交互（设计决策 F2-F5）：
//! - 单击条目 = 回写（写系统剪贴板 → 进本地历史，不重广播）；
//! - hover 显示删除按钮；「清空全部」在顶部；
//! - 新到达条目带一次高亮脉冲（unread 由 App 层计数，见 App.tsx 徽标）；
//! - 空态：区分「接收开启」与「接收关闭」两种引导文案；
//! - 节点分组标题粘性 + 半透明磨砂（沿用全局玻璃视觉）。

import { listen } from "@tauri-apps/api/event";
import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { getErrorCode } from "../../api/clipboard-history";
import {
  clearLanInbox,
  deleteLanInboxEntry,
  getLanInbox,
  getLanSyncStatus,
  LAN_INBOX_UPDATED_EVENT,
  writeLanInboxEntry,
  type LanInboxEntry,
  type LanInboxNode,
} from "../../api/lan-sync";
import { notify } from "../../api/notify";
import { getPlatformInfo } from "../../api/platform";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import { useI18n } from "../../i18n";

/** 条目类型标记（与剪贴板历史同款规则：html > rtf > text > files > image）。 */
function entryKind(entry: LanInboxEntry): "html" | "rtf" | "text" | "files" | "image" {
  if (entry.html) return "html";
  if (entry.rtf) return "rtf";
  if (entry.text) return "text";
  if (entry.filePaths) return "files";
  return "image";
}

/** 条目预览文本（优先 text，其次占位）。 */
function entryPreview(entry: LanInboxEntry): string {
  if (entry.text) return entry.text;
  if (entry.filePaths) return entry.filePaths.join(" · ");
  if (entry.imageMeta) {
    const meta = entry.imageMeta;
    const dims = meta.width && meta.height ? ` (${meta.width}x${meta.height})` : "";
    return `[图片] ${meta.name}${dims}`;
  }
  return "";
}

/** peerId 短号（前 10 字符，用于分组标题补充标识）。 */
function shortPeer(peerId: string): string {
  return peerId.length > 10 ? `${peerId.slice(0, 10)}…` : peerId;
}

/**
 * 移动端不可写：仅含文件路径（契约 mobile 5.2，files-only → `clipboard.write_unsupported`）。
 * 桌面端文件路径可写回系统剪贴板，不受此限制。
 */
function isFilesOnly(entry: LanInboxEntry): boolean {
  return !entry.text && !entry.html && !entry.imageMeta && !!entry.filePaths;
}

interface InboxProps {
  /** 进入/刷新收件箱时调用（App 层重置未读徽标）。 */
  onSeen?: () => void;
}

export function Inbox(props: InboxProps) {
  const { t } = useI18n();
  const [nodes, setNodes] = createSignal<LanInboxNode[]>([]);
  const [receiveEnabled, setReceiveEnabled] = createSignal(true);
  /** 仅初次加载失败保留内联错误态（契约 notify 5.6）；操作反馈走全局通知。 */
  const [loadError, setLoadError] = createSignal("");
  /** 清空确认对话框（替代 window.confirm，0.2.8）。 */
  const [confirmClear, setConfirmClear] = createSignal(false);
  /** 上一次刷新见过的条目 id（用于「新到达」高亮）；首次加载不标记。 */
  const prevSeen = new Set<string>();
  const newIds = new Set<string>();
  /** 是否移动端（null=未加载）：files-only 条目禁止写回（契约 mobile 5.2）。 */
  const [isMobile, setIsMobile] = createSignal<boolean | null>(null);

  onMount(() => {
    void getPlatformInfo()
      .then((info) => setIsMobile(info.isMobile))
      .catch(() => setIsMobile(false));
  });

  async function refresh(initial = false) {
    try {
      const resp = await getLanInbox();
      const next = resp.nodes ?? [];
      if (initial) {
        // 首次加载：仅建立基线，不高亮
        prevSeen.clear();
        newIds.clear();
        for (const node of next) for (const e of node.entries) prevSeen.add(e.id);
      } else {
        newIds.clear();
        for (const node of next) {
          for (const e of node.entries) {
            if (!prevSeen.has(e.id)) newIds.add(e.id);
          }
        }
        prevSeen.clear();
        for (const node of next) for (const e of node.entries) prevSeen.add(e.id);
        props.onSeen?.();
      }
      setNodes(next);
      setLoadError("");
    } catch (err) {
      setLoadError(t(getErrorCode(err) || "lanSync.storage_error"));
    }
  }

  onMount(() => {
    props.onSeen?.();
    void getLanSyncStatus()
      .then((s) => setReceiveEnabled(s.receiveEnabled))
      .catch(() => {});
    void refresh(true);
    // 事件驱动刷新：新消息 / 删除 / 清空
    const unlisten = listen<{ reason: string; id?: string }>(LAN_INBOX_UPDATED_EVENT, () => {
      void refresh(false);
    });
    onCleanup(() => {
      void unlisten.then((fn) => fn());
    });
  });

  async function handleWriteBack(entry: LanInboxEntry) {
    // 移动端：仅文件路径的条目无法写入剪贴板（契约 mobile 5.2），提示而非写回
    if (isMobile() === true && isFilesOnly(entry)) {
      await notify({ level: "warning", code: "lanSync.writeUnsupported" });
      return;
    }
    try {
      await writeLanInboxEntry(entry.id);
      await notify({ level: "success", code: "lanSync.writtenBack" });
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lanSync.entry_not_found" });
    }
  }

  async function handleDelete(entry: LanInboxEntry) {
    try {
      await deleteLanInboxEntry(entry.id);
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lanSync.entry_not_found" });
    }
  }

  async function handleClear() {
    try {
      await clearLanInbox();
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lanSync.storage_error" });
    }
  }

  return (
    <section class="inbox-page">
      <Show when={nodes().length > 0}>
        <div class="history-actions">
          <button type="button" class="btn-ghost" onClick={() => setConfirmClear(true)}>
            {t("lanSync.clear")}
          </button>
        </div>
      </Show>

      <Show
        when={nodes().length > 0}
        fallback={
          <div class="empty">
            {receiveEnabled() ? t("lanSync.empty") : t("lanSync.emptyReceiveOff")}
          </div>
        }
      >
        <div class="inbox-list">
          <For each={nodes()}>
            {(node) => (
              <section class="inbox-node">
                <div class="inbox-node-header">
                  <span class="inbox-node-name">{node.terminalName || shortPeer(node.peerId)}</span>
                  <span class="inbox-node-meta">
                    <span class="inbox-node-peer" title={node.peerId}>
                      {shortPeer(node.peerId)}
                    </span>
                    <span class="inbox-node-count">{t("lanSync.items", { count: node.entries.length })}</span>
                  </span>
                </div>
                <ul class="entry-list">
                  <For each={node.entries}>
                    {(entry) => (
                      <li
                        class={newIds.has(entry.id) ? "entry-card new-flash" : "entry-card"}
                        onClick={(e) => {
                          // 删除按钮区域不触发回写（SolidJS 事件委托下 stopPropagation 不可靠）
                          if ((e.target as HTMLElement).closest(".entry-delete")) return;
                          void handleWriteBack(entry);
                        }}
                      >
                        <span class="entry-kind">{t(`clipboard.kind.${entryKind(entry)}`)}</span>
                        <span class="entry-preview">
                          <span class="text-preview">{entryPreview(entry)}</span>
                        </span>
                        <span class="entry-meta">
                          <span class="inbox-entry-time">{entry.sentAt.slice(11, 16)}</span>
                          <button
                            type="button"
                            class="entry-delete"
                            onClick={(e) => {
                              e.stopPropagation();
                              void handleDelete(entry);
                            }}
                          >
                            {t("lanSync.delete")}
                          </button>
                        </span>
                      </li>
                    )}
                  </For>
                </ul>
              </section>
            )}
          </For>
        </div>
      </Show>

      {loadError() && (
        <div class="settings-row">
          <span class="message error">{loadError()}</span>
        </div>
      )}

      {/* 清空确认（0.2.8：替代 window.confirm，避免宿主原生对话框标题） */}
      <ConfirmDialog
        open={confirmClear()}
        title={t("lanSync.clear")}
        message={t("lanSync.clearConfirm")}
        confirmLabel={t("lanSync.clear")}
        cancelLabel={t("common.cancel")}
        destructive
        onConfirm={() => {
          setConfirmClear(false);
          void handleClear();
        }}
        onCancel={() => setConfirmClear(false)}
      />
    </section>
  );
}
