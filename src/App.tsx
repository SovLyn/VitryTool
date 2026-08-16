import "./App.css";
import { createEffect, createSignal, For, onCleanup, onMount } from "solid-js";
import { LAN_INBOX_UPDATED_EVENT } from "./api/lan-sync";
import { setTrayLabels } from "./api/quick-paste";
import { ClipboardHistory } from "./features/clipboard-history/ClipboardHistory";
import { startClipboardCapture } from "./features/clipboard-history/listener";
import { Inbox } from "./features/lan-sync/Inbox";
import { Settings } from "./features/settings/Settings";
import { useI18n } from "./i18n";
import { listen } from "@tauri-apps/api/event";

type View = "history" | "inbox" | "settings";

/**
 * 主导航项（未来功能在此追加）。
 * 「设置」固定在侧栏底部，不在此列。
 */
const NAV_ITEMS: { view: View; labelKey: string }[] = [
  { view: "history", labelKey: "clipboard.title" },
  { view: "inbox", labelKey: "lanSync.title" },
];

/**
 * 应用入口：左侧标签栏（功能在上、设置固定在底部）+ 右侧内容区。
 * 收件箱页外收到新广播 → 侧栏「收件箱」显示未读徽标；进入收件箱页重置（Inbox onSeen）。
 */
function App() {
  const { t, locale } = useI18n();
  const [view, setView] = createSignal<View>("history");
  const [unread, setUnread] = createSignal(0);

  // 托盘菜单文案跟随语言（契约 quick-paste 5.5，0.2.6；快速开关文案 0.2.7）：
  // 主窗口加载后及语言切换时下发本地化文案；失败仅记日志（托盘仍有默认文案兜底）。
  createEffect(() => {
    void locale(); // 依赖语言变化，触发重新下发
    void setTrayLabels(
      t("tray.showMain"),
      t("tray.quit"),
      t("tray.broadcast"),
      t("tray.receive"),
    ).catch((e) => console.warn("setTrayLabels failed:", e));
  });

  // 应用级剪贴板捕捉：与页面视图无关，启动即监听（设置页 / 主窗口隐藏期间也持续）
  onMount(() => {
    startClipboardCapture();
    const unlisten = listen<{ reason?: string }>(LAN_INBOX_UPDATED_EVENT, (e) => {
      if (e.payload?.reason === "received" && view() !== "inbox") {
        setUnread((n) => n + 1);
      }
    });
    onCleanup(() => {
      void unlisten.then((fn) => fn());
    });
  });

  const toolbarTitle = () =>
    view() === "history"
      ? t("clipboard.title")
      : view() === "inbox"
        ? t("lanSync.title")
        : t("settings.title");

  return (
    <div class="app-shell">
      <aside class="sidebar">
        <nav class="sidebar-nav">
          <For each={NAV_ITEMS}>
            {(item) => (
              <button
                type="button"
                class={view() === item.view ? "nav-item active" : "nav-item"}
                onClick={() => setView(item.view)}
              >
                <span class="nav-item-label">{t(item.labelKey)}</span>
                {item.view === "inbox" && unread() > 0 && (
                  <span class="nav-badge">{unread()}</span>
                )}
              </button>
            )}
          </For>
        </nav>
        <div class="sidebar-bottom">
          <button
            type="button"
            class={view() === "settings" ? "nav-item active" : "nav-item"}
            onClick={() => setView("settings")}
          >
            <span class="nav-item-label">{t("settings.title")}</span>
          </button>
        </div>
      </aside>

      <section class="content">
        <header class="toolbar">
          <span class="toolbar-title">{toolbarTitle()}</span>
        </header>
        <div class="content-body">
          {view() === "history" ? (
            <ClipboardHistory />
          ) : view() === "inbox" ? (
            <Inbox onSeen={() => setUnread(0)} />
          ) : (
            <Settings />
          )}
        </div>
      </section>
    </div>
  );
}

export default App;
