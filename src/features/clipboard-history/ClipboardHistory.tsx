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
  getMaxEntries,
  setEntryFavorite,
  writeClipboardEntry,
  type ClipboardEntry,
} from "../../api/clipboard-history";
import { notify, UNKNOWN_NOTIFY_CODE } from "../../api/notify";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import { StarIcon } from "../../components/StarIcon";
import { useI18n } from "../../i18n";
import { applyCapturedEntry } from "./incremental";
import { CLIPBOARD_UPDATED_EVENT, type ClipboardUpdatedEvent } from "./listener";

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
            loading="lazy"
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
  /** 条数上限缓存（本地增量应用的镜像淘汰需要；随全量刷新同步，见 refresh）。 */
  const [maxEntries, setMaxEntries] = createSignal(0);
  /** 仅初次加载失败保留内联错误态（契约 notify 5.6）；操作反馈走全局通知。 */
  const [loadError, setLoadError] = createSignal("");
  /** 清空确认对话框（替代 window.confirm，0.2.8）。 */
  const [confirmClear, setConfirmClear] = createSignal(false);

  // 收藏区在前（后端已按展示序返回，契约 5.8）；分组仅为渲染划分
  const favorites = () => entries().filter((e) => e.favoritedAt);
  const regular = () => entries().filter((e) => !e.favoritedAt);

  async function refresh() {
    try {
      const [list, max] = await Promise.all([getClipboardHistory(), getMaxEntries()]);
      setEntries(list);
      setMaxEntries(max);
    } catch (err) {
      setLoadError(getErrorCode(err) || String(err));
    }
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;

    void refresh();

    // 应用级监听捕捉成功后广播 → 增量应用（载荷带完整新条目，见 listener.ts）；
    // 无条目载荷（收藏切换）或上限未知（初次加载未完成）时退化为全量刷新
    void listen<ClipboardUpdatedEvent>(CLIPBOARD_UPDATED_EVENT, (e) => {
      const entry = e.payload.entry;
      if (entry && maxEntries() > 0) {
        setEntries(applyCapturedEntry(entries(), entry, maxEntries()));
      } else {
        void refresh();
      }
    }).then((fn) => {
      unlisten = fn;
    });

    onCleanup(() => unlisten?.());
  });

  async function handleCopy(id: string) {
    try {
      await writeClipboardEntry(id);
      await notify({ level: "success", code: "clipboard.copied" });
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || UNKNOWN_NOTIFY_CODE });
    }
  }

  async function handleDelete(id: string) {
    try {
      await deleteClipboardEntry(id);
      await refresh();
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || UNKNOWN_NOTIFY_CODE });
    }
  }

  async function handleToggleFavorite(id: string, favorited: boolean) {
    try {
      await setEntryFavorite(id, favorited);
      // 跨窗同步：两窗共用既有刷新路径（契约 5.8）
      void emit(CLIPBOARD_UPDATED_EVENT, { id });
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || UNKNOWN_NOTIFY_CODE });
    }
  }

  async function handleClear() {
    try {
      await clearClipboardHistory();
      await refresh();
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || UNKNOWN_NOTIFY_CODE });
    }
  }

  return (
    <>
      {loadError() && <p class="message error">{t(loadError()) || loadError()}</p>}

      <div class="history-actions">
        <button type="button" class="btn-ghost" onClick={() => setConfirmClear(true)}>
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

      {entries().length === 0 && !loadError() && <p class="empty">{t("clipboard.empty")}</p>}

      {/* 清空确认（0.2.8：替代 window.confirm，避免宿主原生对话框标题） */}
      <ConfirmDialog
        open={confirmClear()}
        title={t("clipboard.clear")}
        message={t("clipboard.clearConfirm")}
        confirmLabel={t("clipboard.clear")}
        cancelLabel={t("common.cancel")}
        destructive
        onConfirm={() => {
          setConfirmClear(false);
          void handleClear();
        }}
        onCancel={() => setConfirmClear(false)}
      />
    </>
  );
}
