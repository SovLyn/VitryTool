//! 剪贴板历史主界面。
//!
//! 职责（契约见 `docs/api/clipboard-history.md`）：
//! - 挂载时加载历史列表，并监听 `clipboard-history://updated`（应用级监听捕捉成功后广播）刷新；
//! - 列表展示 / 点击回写 / 单条删除 / 清空。
//!
//! 剪贴板捕捉与定时清理已提升到应用级（`listener.ts`），与页面视图无关——
//! 用户在设置页或主窗口隐藏期间复制的内容也会进入历史（0.2.1 修复）。

import { createSignal, For, Show, onCleanup, onMount } from "solid-js";
import { convertFileSrc } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import {
  clearClipboardHistory,
  deleteClipboardEntry,
  getClipboardHistory,
  getErrorCode,
  setEntryFavorite,
  writeClipboardEntry,
  type ClipboardEntry,
} from "../../api/clipboard-history";
import { StarIcon } from "../../components/StarIcon";
import { useI18n } from "../../i18n";
import { CLIPBOARD_UPDATED_EVENT } from "./listener";

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
 * 回退为占位文本，而非显示裂图。收藏条目带左侧强调条与星标（契约 5.8）。
 */
function EntryCard(props: {
  entry: ClipboardEntry;
  onCopy: (id: string) => void;
  onDelete: (id: string) => void;
  onToggleFavorite: (id: string, favorited: boolean) => void;
}) {
  const { t } = useI18n();
  const [imgFailed, setImgFailed] = createSignal(false);
  const kind = entryKind(props.entry);
  const missing = props.entry.image?.missing ?? false;
  const favorited = !!props.entry.favoritedAt;

  return (
    <li
      class={favorited ? "entry-card favorited" : "entry-card"}
      onClick={() => props.onCopy(props.entry.id)}
    >
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
        class={favorited ? "entry-star favorited" : "entry-star"}
        aria-label={favorited ? t("clipboard.unfavorite") : t("clipboard.favorite")}
        aria-pressed={favorited}
        onClick={(e) => {
          e.stopPropagation();
          props.onToggleFavorite(props.entry.id, !favorited);
        }}
      >
        <StarIcon filled={favorited} />
      </button>
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

  // 收藏区在前（后端已按展示序返回，契约 5.8）；分组仅为渲染划分
  const favorites = () => entries().filter((e) => e.favoritedAt);
  const regular = () => entries().filter((e) => !e.favoritedAt);

  async function refresh() {
    try {
      setEntries(await getClipboardHistory());
    } catch (err) {
      setError(getErrorCode(err) || String(err));
    }
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;

    void refresh();

    // 应用级监听捕捉成功后广播 → 刷新列表
    void listen(CLIPBOARD_UPDATED_EVENT, () => void refresh()).then((fn) => {
      unlisten = fn;
    });

    onCleanup(() => unlisten?.());
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

  async function handleToggleFavorite(id: string, favorited: boolean) {
    try {
      await setEntryFavorite(id, favorited);
      // 跨窗同步：两窗共用既有刷新路径（契约 5.8）
      void emit(CLIPBOARD_UPDATED_EVENT, { id });
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
        <Show when={favorites().length > 0}>
          <li class="entry-section-title">
            {t("clipboard.favorites")} ({favorites().length})
          </li>
        </Show>
        <For each={favorites()}>
          {(entry) => (
            <EntryCard
              entry={entry}
              onCopy={(id) => void handleCopy(id)}
              onDelete={(id) => void handleDelete(id)}
              onToggleFavorite={(id, fav) => void handleToggleFavorite(id, fav)}
            />
          )}
        </For>
        <For each={regular()}>
          {(entry) => (
            <EntryCard
              entry={entry}
              onCopy={(id) => void handleCopy(id)}
              onDelete={(id) => void handleDelete(id)}
              onToggleFavorite={(id, fav) => void handleToggleFavorite(id, fav)}
            />
          )}
        </For>
      </ul>

      {entries().length === 0 && !error() && <p class="empty">{t("clipboard.empty")}</p>}
    </>
  );
}
