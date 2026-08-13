//! 剪贴板历史主界面。
//!
//! 职责（契约见 `docs/api/clipboard-history.md`）：
//! - 挂载时自动 `startListening` + 注册 `onClipboardChange`（启动即监听，D10）；
//! - 收到变化事件 → `captureClipboard` → 刷新列表；
//! - 前台定时（5 分钟）发起 `cleanupOrphanImages` 兜底清理（D6）；
//! - 列表展示 / 点击回写 / 单条删除 / 清空。
//! 卸载时注销监听与定时器。设置（语言/主题/条数）在独立设置页 `Settings`。

import { createSignal, For, onCleanup, onMount } from "solid-js";
import { convertFileSrc } from "@tauri-apps/api/core";
import { onClipboardChange, startListening } from "tauri-plugin-clipboard-x-api";
import {
  captureClipboard,
  cleanupOrphanImages,
  clearClipboardHistory,
  deleteClipboardEntry,
  getClipboardHistory,
  getErrorCode,
  writeClipboardEntry,
  type ClipboardEntry,
} from "../../api/clipboard-history";
import { useI18n } from "../../i18n";

/** 定时兜底清理间隔（固定，不暴露设置项，D6）。 */
const SWEEP_INTERVAL_MS = 5 * 60 * 1000;

type EntryKind = "text" | "image" | "html" | "rtf" | "files";

function entryKind(entry: ClipboardEntry): EntryKind {
  if (entry.image) return "image";
  if (entry.html) return "html";
  if (entry.rtf) return "rtf";
  if (entry.files) return "files";
  return "text";
}

function formatTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/**
 * 单条历史卡片（独立组件以持有局部信号）。
 *
 * 点击卡片回写剪贴板；图片缩略图加载失败（asset 协议未命中/文件异常）时
 * 回退为占位文本，而非显示裂图。
 */
function EntryCard(props: {
  entry: ClipboardEntry;
  onCopy: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  const { t } = useI18n();
  const [imgFailed, setImgFailed] = createSignal(false);
  const kind = entryKind(props.entry);
  const missing = props.entry.image?.missing ?? false;

  return (
    <li class="entry-card" onClick={() => props.onCopy(props.entry.id)}>
      <div class="entry-preview">
        {kind === "image" && !missing && !imgFailed() && props.entry.image && (
          <img
            src={convertFileSrc(props.entry.image.path)}
            alt={t("clipboard.image")}
            onError={() => setImgFailed(true)}
          />
        )}
        {kind === "image" && (missing || imgFailed()) && (
          <span class="missing">
            {missing ? t("clipboard.missingImage") : t("clipboard.imageLoadFailed")}
          </span>
        )}
        {kind === "text" && <span class="text-preview">{props.entry.text}</span>}
        {kind === "html" && <span class="text-preview">{props.entry.text ?? props.entry.html}</span>}
        {kind === "rtf" && <span class="text-preview">{props.entry.text ?? props.entry.rtf}</span>}
        {kind === "files" && props.entry.files && (
          <span class="text-preview">{props.entry.files.paths.join("; ")}</span>
        )}
      </div>
      <div class="entry-meta">
        <span class="entry-kind">{t(`clipboard.kind.${kind}`)}</span>
        <time>{formatTime(props.entry.capturedAt)}</time>
      </div>
      <button
        type="button"
        class="entry-delete"
        onClick={(e) => {
          e.stopPropagation();
          props.onDelete(props.entry.id);
        }}
      >
        {t("clipboard.delete")}
      </button>
    </li>
  );
}

export function ClipboardHistory() {
  const { t } = useI18n();
  const [entries, setEntries] = createSignal<ClipboardEntry[]>([]);
  const [error, setError] = createSignal("");
  const [notice, setNotice] = createSignal("");

  async function refresh() {
    try {
      setEntries(await getClipboardHistory());
    } catch (err) {
      setError(getErrorCode(err) || String(err));
    }
  }

  /** 剪贴板变化事件：捕捉后刷新列表。 */
  async function handleClipboardChanged() {
    try {
      await captureClipboard();
      await refresh();
    } catch (err) {
      setError(getErrorCode(err) || String(err));
    }
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    void refresh();

    // 启动即监听（D10）；回写是否触发监听按项目经验不触发，实现后实测（契约 5.5）
    void startListening()
      .then(() => (disposed ? undefined : onClipboardChange(handleClipboardChanged)))
      .then((fn) => {
        unlisten = fn;
      })
      .catch((err) => setError(getErrorCode(err) || String(err)));

    // 定时兜底清理：由前台发起（D6、ADR 0001）
    const timer = setInterval(() => {
      void cleanupOrphanImages().catch((err) => setError(getErrorCode(err) || String(err)));
    }, SWEEP_INTERVAL_MS);

    onCleanup(() => {
      disposed = true;
      unlisten?.();
      clearInterval(timer);
    });
  });

  async function handleCopy(id: string) {
    try {
      await writeClipboardEntry(id);
      setNotice(t("clipboard.copied"));
    } catch (err) {
      setError(getErrorCode(err) || String(err));
    }
  }

  async function handleDelete(id: string) {
    try {
      await deleteClipboardEntry(id);
      await refresh();
    } catch (err) {
      setError(getErrorCode(err) || String(err));
    }
  }

  async function handleClear() {
    if (!window.confirm(t("clipboard.clearConfirm"))) return;
    try {
      await clearClipboardHistory();
      await refresh();
    } catch (err) {
      setError(getErrorCode(err) || String(err));
    }
  }

  return (
    <>
      {error() && <p class="message error">{t(error()) || error()}</p>}
      {notice() && <p class="message notice">{notice()}</p>}

      <div class="history-actions">
        <button type="button" class="btn-ghost" onClick={handleClear}>
          {t("clipboard.clear")}
        </button>
      </div>

      <ul class="entry-list">
        <For each={entries()}>
          {(entry) => (
            <EntryCard
              entry={entry}
              onCopy={(id) => void handleCopy(id)}
              onDelete={(id) => void handleDelete(id)}
            />
          )}
        </For>
      </ul>

      {entries().length === 0 && !error() && <p class="empty">{t("clipboard.empty")}</p>}
    </>
  );
}
