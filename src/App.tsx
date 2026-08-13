import "./App.css";
import { ClipboardHistory } from "./features/clipboard-history/ClipboardHistory";
import { locales, useI18n, type Locale } from "./i18n";

/**
 * 应用入口组件。
 *
 * 首个功能（剪贴板历史）已落地：主界面为历史列表；
 * 脚手架 greet 演示已移除。
 */
function App() {
  const { locale, setLocale } = useI18n();

  return (
    <main class="app-root">
      <div class="lang-switcher">
        {locales.map((l) => (
          <button
            type="button"
            class={locale() === l ? "active" : ""}
            onClick={() => setLocale(l as Locale)}
          >
            {l}
          </button>
        ))}
      </div>
      <ClipboardHistory />
    </main>
  );
}

export default App;
