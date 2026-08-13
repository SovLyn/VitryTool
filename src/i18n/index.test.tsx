import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { I18nProvider, useI18n } from "./index";

const LOCALE_KEY = "vitrytool.locale";

/** 消费 i18n 的最小组件。 */
function Consumer() {
  const { t } = useI18n();
  return <div>{t("settings.title")}</div>;
}

describe("I18nProvider 跨窗口语言同步", () => {
  afterEach(() => {
    cleanup();
    localStorage.removeItem(LOCALE_KEY);
  });

  it("默认语言为 zh-CN", () => {
    render(() => (
      <I18nProvider>
        <Consumer />
      </I18nProvider>
    ));
    expect(screen.getByText("设置")).toBeTruthy();
  });

  it("其他窗口写入语言后，storage 事件触发本窗口跟随", () => {
    render(() => (
      <I18nProvider>
        <Consumer />
      </I18nProvider>
    ));
    expect(screen.getByText("设置")).toBeTruthy();

    // 模拟主窗口切换语言（写入 localStorage，同源小窗收到 storage 事件）
    localStorage.setItem(LOCALE_KEY, "en-US");
    fireEvent(
      window,
      new StorageEvent("storage", { key: LOCALE_KEY, newValue: "en-US" }),
    );
    expect(screen.getByText("Settings")).toBeTruthy();
  });

  it("storage 事件携带无效语言时忽略", () => {
    render(() => (
      <I18nProvider>
        <Consumer />
      </I18nProvider>
    ));
    fireEvent(window, new StorageEvent("storage", { key: LOCALE_KEY, newValue: "fr-FR" }));
    expect(screen.getByText("设置")).toBeTruthy();
  });
});
