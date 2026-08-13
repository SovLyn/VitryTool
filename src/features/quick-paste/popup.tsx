//! 快速粘贴小屏入口（独立 HTML 页面 `popup.html`，Vite 多入口）。
//!
//! 复用主应用的视觉系统：`../theme`（副作用应用亮/暗主题）与 `../App.css`（CSS 变量）。

import "../../theme"; // 副作用：模块加载即应用 data-theme（首帧无闪烁）
import "../../App.css";
import "./popup.css";
import { render } from "solid-js/web";
import { I18nProvider } from "../../i18n";
import { QuickPastePopup } from "./QuickPastePopup";

const root = document.getElementById("root")!;

render(
  () => (
    <I18nProvider>
      <QuickPastePopup />
    </I18nProvider>
  ),
  root,
);
