/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";
import { I18nProvider } from "./i18n";
import { ThemeProvider } from "./theme";

// ---- 开屏（#boot-loader，index.html 内联）退场逻辑 ----
//
// 开屏在 HTML 解析时即绘制（防白屏）；本模块在 App 首次渲染完成后触发退场：
// 「就绪」与「最短展示时间（自页面到达计时）」取晚者——生产启动快时保证
// logo 完整浮现一遍（不闪一下），dev 冷启动慢时开屏持续覆盖直到就绪。
// 退场 = 加 .leaving（opacity 淡出，reduced-motion 缩短）→ 过渡结束移除节点。
const BOOT_MIN_MS = 500;
const BOOT_FADE_MS = 400;
const BOOT_FADE_REDUCED_MS = 150;

/** 页面到达时刻（≈ 开屏绘制时刻）；拿不到导航计时时退回模块求值时刻。 */
function bootStartTime(): number {
  const nav = performance.getEntriesByType("navigation")[0] as
    | PerformanceNavigationTiming
    | undefined;
  return nav && nav.responseEnd > 0 ? nav.responseEnd : performance.now();
}

function hideBootLoader(): void {
  const el = document.getElementById("boot-loader");
  if (!el) return;
  const elapsed = performance.now() - bootStartTime();
  const delay = Math.max(0, BOOT_MIN_MS - elapsed);
  window.setTimeout(() => {
    if (!document.getElementById("boot-loader")) return; // 已移除（如 HMR 重复执行）
    el.classList.add("leaving");
    const fadeMs = window.matchMedia("(prefers-reduced-motion: reduce)").matches
      ? BOOT_FADE_REDUCED_MS
      : BOOT_FADE_MS;
    window.setTimeout(() => el.remove(), fadeMs);
  }, delay);
}

render(
  () => (
    <I18nProvider>
      <ThemeProvider>
        <App />
      </ThemeProvider>
    </I18nProvider>
  ),
  document.getElementById("root") as HTMLElement
);

// Solid render 为同步渲染，返回时应用 DOM 已就位；随后淡出开屏衔接应用首帧
hideBootLoader();
