import "./App.css";
import { createEffect, createSignal, For, onCleanup, onMount } from "solid-js";
import { LAN_INBOX_UPDATED_EVENT } from "./api/lan-sync";
import { getPlatformInfo } from "./api/platform";
import { setTrayLabels } from "./api/quick-paste";
import { NotificationProvider } from "./components/NotificationProvider";
import { ClipboardHistory } from "./features/clipboard-history/ClipboardHistory";
import { startClipboardCapture } from "./features/clipboard-history/listener";
import { Inbox } from "./features/lan-sync/Inbox";
import { Settings } from "./features/settings/Settings";
import { useI18n } from "./i18n";
import { listen } from "@tauri-apps/api/event";

type View = "history" | "inbox" | "settings";

/** 导航图标（内联 SVG path，移动端底部 tab 显示，桌面隐藏，见 App.css 断点）。 */
const NAV_ICONS: Record<View, string> = {
  history: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
  inbox: "M21 3H3a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h18a1 1 0 0 0 1-1V4a1 1 0 0 0-1-1zM3 10l9 6 9-6",
  settings: "M4 7h9M17 7h3M4 12h3M11 12h9M4 17h9M17 17h3",
};

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
  /** 平台信息（null = 未加载）：移动端隔离桌面功能（契约 mobile 5.1）。 */
  const [isMobile, setIsMobile] = createSignal<boolean | null>(null);

  // 平台识别：移动端不启动剪贴板监听、不下发托盘文案（无托盘/监听，契约 mobile 5.1）
  onMount(() => {
    void getPlatformInfo()
      .then((info) => setIsMobile(info.isMobile))
      .catch(() => setIsMobile(false)); // 失败按桌面 fail-open，不阻塞功能
  });

  // 托盘菜单文案跟随语言（契约 quick-paste 5.5，0.2.6；快速开关文案 0.2.7）：
  // 主窗口加载后及语言切换时下发本地化文案；失败仅记日志（托盘仍有默认文案兜底）。
  // 移动端无托盘：isMobile !== false（未加载或移动端）时不调用。
  createEffect(() => {
    void locale(); // 依赖语言变化，触发重新下发
    if (isMobile() !== false) return;
    void setTrayLabels(
      t("tray.showMain"),
      t("tray.quit"),
      t("tray.broadcast"),
      t("tray.receive"),
    ).catch((e) => console.warn("setTrayLabels failed:", e));
  });

  // 应用级剪贴板捕捉：与页面视图无关，启动即监听（设置页 / 主窗口隐藏期间也持续）。
  // 桌面专属：移动端不监听（契约 mobile 5.1），平台识别完成后再启动。
  onMount(() => {
    void getPlatformInfo().then((info) => {
      if (!info.isMobile) startClipboardCapture();
    });
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
    <>
      {/* 全局通知（0.2.8）：仅主窗口挂载；小屏 popup 不渲染（契约 notify 5.6） */}
      <NotificationProvider />
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
                  <span class="nav-icon" aria-hidden="true">
                    <svg
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.8"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    >
                      <path d={NAV_ICONS[item.view]} />
                    </svg>
                  </span>
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
              <span class="nav-icon" aria-hidden="true">
                <svg
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.8"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <path d={NAV_ICONS.settings} />
                </svg>
              </span>
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
    </>
  );
}

export default App;
