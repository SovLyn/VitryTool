//! 主题系统（亮色 / 暗色 / 跟随系统）。
//!
//! - `Theme` 为用户选择，`ResolvedTheme` 为实际生效的亮/暗（`system` 时跟随系统）。
//! - 持久化到 `localStorage`（纯前端展示关注点，后端不感知）。
//! - 通过 `<html data-theme="light|dark">` 驱动 CSS 变量切换（见 `App.css`）。
//! - 模块加载时同步应用初始主题，避免首帧闪烁（FOUC）。

import { createContext, createEffect, createSignal, onCleanup, useContext, type ParentProps } from "solid-js";

export type Theme = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "vitrytool.theme";

function isTheme(value: string): value is Theme {
  return value === "light" || value === "dark" || value === "system";
}

function loadTheme(): Theme {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    return saved && isTheme(saved) ? saved : "system";
  } catch {
    return "system";
  }
}

function systemTheme(): ResolvedTheme {
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function resolve(theme: Theme): ResolvedTheme {
  return theme === "system" ? systemTheme() : theme;
}

function applyTheme(resolved: ResolvedTheme) {
  document.documentElement.setAttribute("data-theme", resolved);
}

// 模块加载即应用初始主题，避免首帧闪烁。
applyTheme(resolve(loadTheme()));

export interface ThemeContextValue {
  /** 用户选择的主题（light / dark / system）。 */
  theme: () => Theme;
  /** 实际生效的亮/暗主题。 */
  resolved: () => ResolvedTheme;
  /** 设置主题并持久化。 */
  setTheme: (theme: Theme) => void;
}

const ThemeContext = createContext<ThemeContextValue>();

export function ThemeProvider(props: ParentProps) {
  const [theme, setThemeSignal] = createSignal<Theme>(loadTheme());
  const [resolved, setResolved] = createSignal<ResolvedTheme>(resolve(theme()));

  // 主题变化 → 解析并应用到 <html>
  createEffect(() => {
    const r = resolve(theme());
    setResolved(r);
    applyTheme(r);
  });

  // 跟随系统：theme = "system" 时监听系统亮暗切换
  createEffect(() => {
    if (theme() !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      setResolved(systemTheme());
      applyTheme(systemTheme());
    };
    mq.addEventListener("change", onChange);
    onCleanup(() => mq.removeEventListener("change", onChange));
  });

  const setTheme = (next: Theme) => {
    setThemeSignal(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // 忽略持久化失败（隐私模式等），仅影响下次启动
    }
  };

  const value: ThemeContextValue = { theme, resolved, setTheme };
  return <ThemeContext.Provider value={value}>{props.children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme 必须在 ThemeProvider 内使用");
  return ctx;
}
