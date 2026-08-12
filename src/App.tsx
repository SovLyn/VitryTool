import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import { useI18n, locales, type Locale } from "./i18n";

/**
 * 应用入口组件。
 *
 * 目前为脚手架演示（greet 前后端链路 + i18n 切换），
 * 首个功能落地后由 `src/features/<feature>/` 中的功能页面替换。
 */
function App() {
  const { t, locale, setLocale } = useI18n();
  const [greetMsg, setGreetMsg] = createSignal("");
  const [name, setName] = createSignal("");

  async function greet() {
    // 前端唯一与后端通信的方式是 invoke（见 docs/architecture.md 第 2 节）
    await invoke("greet", { name: name() });
    setGreetMsg(t("demo.result", { name: name() || "?" }));
  }

  return (
    <main class="container">
      <h1>{t("app.title")}</h1>
      <p class="tagline">{t("app.tagline")}</p>

      <div class="row lang-switcher">
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

      <h2>{t("demo.greeting")}</h2>
      <p>{t("demo.hint")}</p>

      <form
        class="row"
        onSubmit={(e) => {
          e.preventDefault();
          greet();
        }}
      >
        <input
          id="greet-input"
          onChange={(e) => setName(e.currentTarget.value)}
          placeholder={t("demo.enterName")}
        />
        <button type="submit">{t("demo.greet")}</button>
      </form>
      <p>{greetMsg()}</p>
    </main>
  );
}

export default App;
