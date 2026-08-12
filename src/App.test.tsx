import { describe, it, expect, vi } from "vitest";
import { render } from "@solidjs/testing-library";
import { I18nProvider } from "./i18n";
import App from "./App";

// 渲染不触发 invoke；mock 确保测试环境无 Tauri 宿主时也不炸
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => "mocked"),
}));

describe("App", () => {
  it("渲染 i18n 后的标题（zh-CN 默认）", () => {
    const { getByText } = render(() => (
      <I18nProvider>
        <App />
      </I18nProvider>
    ));
    expect(getByText("欢迎使用 VitryTool")).toBeTruthy();
  });

  it("渲染语言切换按钮", () => {
    const { getByText } = render(() => (
      <I18nProvider>
        <App />
      </I18nProvider>
    ));
    expect(getByText("zh-CN")).toBeTruthy();
    expect(getByText("en-US")).toBeTruthy();
  });
});
