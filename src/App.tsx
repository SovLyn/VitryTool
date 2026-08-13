import "./App.css";
import { createSignal, For, onMount } from "solid-js";
import { ClipboardHistory } from "./features/clipboard-history/ClipboardHistory";
import { startClipboardCapture } from "./features/clipboard-history/listener";
import { Settings } from "./features/settings/Settings";
import { useI18n } from "./i18n";

type View = "history" | "settings";

/**
 * 主导航项（未来功能在此追加）。
 * 「设置」固定在侧栏底部，不在此列。
 */
const NAV_ITEMS: { view: View; labelKey: string }[] = [
  { view: "history", labelKey: "clipboard.title" },
];

/**
 * 应用入口：左侧标签栏（功能在上、设置固定在底部）+ 右侧内容区。
 */
function App() {
  const { t } = useI18n();
  const [view, setView] = createSignal<View>("history");

  // 应用级剪贴板捕捉：与页面视图无关，启动即监听（设置页 / 主窗口隐藏期间也持续）
  onMount(() => startClipboardCapture());

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
                {t(item.labelKey)}
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
            {t("settings.title")}
          </button>
        </div>
      </aside>

      <section class="content">
        <header class="toolbar">
          <span class="toolbar-title">
            {view() === "history" ? t("clipboard.title") : t("settings.title")}
          </span>
        </header>
        <div class="content-body">
          {view() === "history" ? <ClipboardHistory /> : <Settings />}
        </div>
      </section>
    </div>
  );
}

export default App;
