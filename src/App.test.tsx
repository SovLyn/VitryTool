import { describe, it, expect, vi } from "vitest";
import { render } from "@solidjs/testing-library";
import { I18nProvider } from "./i18n";
import App from "./App";

// 剪贴板历史依赖 Tauri 宿主能力，测试中全部 mock（组件纯渲染验证）
vi.mock("tauri-plugin-clipboard-x-api", () => ({
  startListening: vi.fn(async () => undefined),
  onClipboardChange: vi.fn(async () => () => undefined),
}));
vi.mock("./api/clipboard-history", () => ({
  captureClipboard: vi.fn(async () => null),
  getClipboardHistory: vi.fn(async () => []),
  cleanupOrphanImages: vi.fn(async () => ({ removed: 0 })),
  clearClipboardHistory: vi.fn(async () => undefined),
  deleteClipboardEntry: vi.fn(async () => undefined),
  getErrorCode: vi.fn(() => ""),
  getMaxEntries: vi.fn(async () => 64),
  setMaxEntries: vi.fn(async (n: number) => ({ maxEntries: n, evicted: 0 })),
  writeClipboardEntry: vi.fn(async () => undefined),
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => undefined),
  convertFileSrc: vi.fn((p: string) => p),
}));
// 主题模块含 matchMedia/localStorage 副作用，测试中 mock 掉 useTheme 供设置页使用
vi.mock("./theme", () => ({
  useTheme: () => ({
    theme: () => "system" as const,
    resolved: () => "light" as const,
    setTheme: vi.fn(),
  }),
}));

function renderApp() {
  return render(() => (
    <I18nProvider>
      <App />
    </I18nProvider>
  ));
}

describe("App", () => {
  it("渲染剪贴板历史（侧栏导航 + 顶栏标题，zh-CN 默认）", () => {
    const { getAllByText } = renderApp();
    expect(getAllByText("剪贴板历史").length).toBeGreaterThan(0);
  });

  it("侧栏底部有「设置」入口", () => {
    const { getByText } = renderApp();
    expect(getByText("设置")).toBeTruthy();
  });

  it("空历史时显示引导文案", () => {
    const { getByText } = renderApp();
    expect(getByText(/暂无剪贴记录/)).toBeTruthy();
  });
});
