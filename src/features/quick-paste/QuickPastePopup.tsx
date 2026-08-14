//! 快速粘贴小屏（Quick Paste Popup）——独立窗口页面组件。
//!
//! 数据流（契约 `docs/api/quick-paste.md` 第 5.3 节）：
//! - 挂载时注册 `show` / `release` 事件监听，并调用 `quickPasteReady` 握手；
//! - `show`：拉取剪贴板历史（复用 clipboard-history 命令），选中第一项（最新）；
//! - `wheel` / ↑↓ 切换选中（边界 clamp，不循环）；
//! - `release`：回写选中项（`writeClipboardEntry`）→ `quickPasteClose`；
//! - `Esc`：取消，不回写直接关闭。

import { createEffect, createSignal, For, onCleanup, onMount } from "solid-js";
import { convertFileSrc } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import {
  captureClipboard,
  getClipboardHistory,
  getErrorCode,
  setEntryFavorite,
  writeClipboardEntry,
  type ClipboardEntry,
} from "../../api/clipboard-history";
import { StarIcon } from "../../components/StarIcon";
import { quickPasteClose, quickPasteReady } from "../../api/quick-paste";
import { CLIPBOARD_UPDATED_EVENT } from "../clipboard-history/listener";
import { useI18n } from "../../i18n";

/** 事件载荷：会话 id（后端用于防过期回调误关新会话）。 */
export interface SessionPayload {
  session: number;
}

/** 选中索引边界收敛：超出 [0, len-1] 时 clamp（不循环，契约 5.3）。 */
export function clampIndex(index: number, len: number): number {
  if (len <= 0) return 0;
  return Math.min(Math.max(index, 0), len - 1);
}

function entryKind(entry: ClipboardEntry): "text" | "image" | "html" | "rtf" | "files" {
  if (entry.image) return "image";
  if (entry.html) return "html";
  if (entry.rtf) return "rtf";
  if (entry.files) return "files";
  return "text";
}

/** 单条预览（popup 内紧凑版）：图片缩略图 / 单行文本截断；星标按钮切换收藏（契约 5.8）。 */
function PopupItem(props: {
  entry: ClipboardEntry;
  active: boolean;
  onToggleFavorite: (id: string, favorited: boolean) => void;
}) {
  const { t } = useI18n();
  const kind = entryKind(props.entry);
  const missing = props.entry.image?.missing ?? false;
  const favorited = !!props.entry.favoritedAt;

  let preview: string | undefined;
  if (kind === "text") preview = props.entry.text;
  else if (kind === "html" || kind === "rtf") preview = props.entry.text ?? props.entry.html ?? props.entry.rtf;
  else if (kind === "files") preview = props.entry.files?.paths.join("; ");

  return (
    <li class={props.active ? "qp-item active" : "qp-item"} data-kind={kind}>
      {kind === "image" && !missing && props.entry.image && (
        <span class="qp-item-preview">
          <img src={convertFileSrc(props.entry.image.path)} alt={t("clipboard.image")} />
        </span>
      )}
      {kind === "image" && missing && <span class="qp-item-preview">{t("clipboard.missingImage")}</span>}
      {preview !== undefined && <span class="qp-item-preview">{preview}</span>}
      <button
        type="button"
        class={favorited ? "qp-star favorited" : "qp-star"}
        aria-label={favorited ? t("clipboard.unfavorite") : t("clipboard.favorite")}
        aria-pressed={favorited}
        onClick={(e) => {
          e.stopPropagation();
          props.onToggleFavorite(props.entry.id, !favorited);
        }}
      >
        <StarIcon filled={favorited} />
      </button>
      <span class="qp-item-kind">{t(`clipboard.kind.${kind}`)}</span>
    </li>
  );
}

export function QuickPastePopup() {
  const { t } = useI18n();
  const [entries, setEntries] = createSignal<ClipboardEntry[]>([]);
  const [selected, setSelected] = createSignal(0);
  const [visible, setVisible] = createSignal(false);
  const [error, setError] = createSignal("");
  let sessionRef = 0;
  let listEl: HTMLUListElement | undefined;

  const count = () => entries().length;
  const currentIndex = () => clampIndex(selected(), count());

  async function handleShow(payload: SessionPayload) {
    sessionRef = payload.session;
    setError("");
    // 每次 show 重播进入动画（先复位再双 rAF 生效）
    setVisible(false);
    requestAnimationFrame(() => requestAnimationFrame(() => setVisible(true)));
    // 先补一次捕捉（主窗口可能在设置页 / 隐藏期间未捕捉到最新复制），再拉取列表
    try {
      await captureClipboard();
    } catch {
      // 补捕捉失败不阻塞展示（历史可能已包含最新内容）
    }
    await refreshEntries(false);
  }

  /** 拉取最新历史并刷新列表；keepSelection 时保持当前选中条目（被淘汰则重置到第一项）。 */
  async function refreshEntries(keepSelection: boolean) {
    const prevId = keepSelection ? entries()[currentIndex()]?.id : undefined;
    try {
      const list = await getClipboardHistory();
      setEntries(list);
      if (prevId) {
        const idx = list.findIndex((e) => e.id === prevId);
        setSelected(idx >= 0 ? idx : 0);
      } else {
        setSelected(0);
      }
    } catch (err) {
      setError(getErrorCode(err) || String(err));
      setEntries([]);
    }
  }

  async function handleHistoryUpdated() {
    // 小屏激活期间主窗口捕捉到新内容 → 实时刷新（保持当前选中条目）
    if (visible()) await refreshEntries(true);
  }

  async function handleRelease(payload: SessionPayload) {
    if (payload.session !== sessionRef) return; // 过期会话忽略
    const entry = entries()[currentIndex()];
    if (entry) {
      try {
        await writeClipboardEntry(entry.id);
      } catch (err) {
        // 回写失败也照常关闭（后端有兜底隐藏）；错误仅在会话内可见
        setError(getErrorCode(err) || String(err));
      }
    }
    await closeSession();
  }

  /** 收藏/取消收藏选中条目（F 键或星标按钮触发）；emit 后经既有事件刷新，保持当前选中。 */
  async function handleToggleFavorite(id: string, favorited: boolean) {
    try {
      await setEntryFavorite(id, favorited);
      void emit(CLIPBOARD_UPDATED_EVENT, { id });
    } catch (err) {
      setError(getErrorCode(err) || String(err));
    }
  }

  async function closeSession() {
    try {
      await quickPasteClose(sessionRef);
    } catch {
      // 关闭失败由后端兜底隐藏
    }
    setVisible(false);
  }

  onMount(() => {
    let unlistenShow: (() => void) | undefined;
    let unlistenRelease: (() => void) | undefined;
    let unlistenUpdated: (() => void) | undefined;

    void listen<SessionPayload>("quick-paste://show", (e) => void handleShow(e.payload)).then(
      (fn) => (unlistenShow = fn),
    );
    void listen<SessionPayload>("quick-paste://release", (e) => void handleRelease(e.payload)).then(
      (fn) => (unlistenRelease = fn),
    );
    // 小屏激活期间实时刷新（主窗口捕捉到新复制后广播）
    void listen(CLIPBOARD_UPDATED_EVENT, () => void handleHistoryUpdated()).then(
      (fn) => (unlistenUpdated = fn),
    );
    // 握手：后端如有挂起的按下事件（首次按下时 WebView 未加载完）则补发 show
    void quickPasteReady().catch(() => {});

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        void closeSession();
        return;
      }
      if (count() === 0) return;
      if (e.key === "f" || e.key === "F") {
        // F：收藏/取消收藏选中条目（契约 5.8）
        const entry = entries()[currentIndex()];
        if (entry) void handleToggleFavorite(entry.id, !entry.favoritedAt);
      } else if (e.key === "ArrowDown") setSelected((i) => clampIndex(i + 1, count()));
      else if (e.key === "ArrowUp") setSelected((i) => clampIndex(i - 1, count()));
    };
    window.addEventListener("keydown", onKeyDown);

    onCleanup(() => {
      unlistenShow?.();
      unlistenRelease?.();
      unlistenUpdated?.();
      window.removeEventListener("keydown", onKeyDown);
    });
  });

  function onWheel(e: WheelEvent) {
    if (count() === 0) return;
    setSelected((i) => clampIndex(i + (e.deltaY > 0 ? 1 : -1), count()));
  }

  // 选中项变化时滚动到可见区域（jsdom 等环境无 scrollIntoView 时跳过）
  createEffect(() => {
    const items = listEl?.querySelectorAll<HTMLLIElement>("li");
    const el = items?.[currentIndex()];
    if (el && typeof el.scrollIntoView === "function") {
      el.scrollIntoView({ block: "nearest" });
    }
  });

  return (
    <div class="qp-overlay">
      <div class={visible() ? "qp-card enter" : "qp-card"} onWheel={onWheel}>
        <header class="qp-header">
          <span class="qp-title">{t("quickPaste.popupTitle")}</span>
        </header>
        {error() && <p class="message error">{t(error()) || error()}</p>}
        <ul ref={listEl} class="qp-list">
          <For each={entries()}>
            {(entry, i) => (
              <PopupItem
                entry={entry}
                active={i() === currentIndex()}
                onToggleFavorite={(id, fav) => void handleToggleFavorite(id, fav)}
              />
            )}
          </For>
        </ul>
        {count() === 0 && !error() && <p class="qp-empty">{t("clipboard.empty")}</p>}
        <footer class="qp-footer">{t("quickPaste.popupHint")}</footer>
      </div>
    </div>
  );
}
