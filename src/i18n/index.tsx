//! 前端国际化基建。
//!
//! 开发阶段仅支持 `zh-CN` 与 `en-US`（见 docs/architecture.md 第 6 节）。
//! 新增文案必须同时写入两份语言资源：`locales/zh-CN.json` 与 `locales/en-US.json`。
//!
//! 实现说明：`@solid-primitives/i18n` 2.x 为纯函数式 API，
//! 本模块用其 `flatten()` 将嵌套字典拍平为「点分路径 key」，
//! 并封装轻量插值（`{name}`），对外暴露 `t("a.b.c", { params })`。

import { flatten } from "@solid-primitives/i18n";
import {
  createContext,
  createSignal,
  onCleanup,
  onMount,
  useContext,
  type ParentProps,
} from "solid-js";
import zhCN from "./locales/zh-CN.json";
import enUS from "./locales/en-US.json";

export type Locale = "zh-CN" | "en-US";

/** 支持的语言列表（开发阶段固定两种）。 */
export const locales: Locale[] = ["zh-CN", "en-US"];

/** 拍平后的字典：key 为点分路径（如 `demo.greeting`）。 */
const flatDictionaries = {
  "zh-CN": flatten(zhCN),
  "en-US": flatten(enUS),
} as const;

/** 字典结构类型（以 zh-CN 为基准）。 */
export type Dictionary = typeof zhCN;

/** 翻译函数签名：点分路径 + 可选插值参数。 */
export type TFunction = (
  key: string,
  params?: Record<string, string | number | boolean>,
) => string;

export interface I18nContextValue {
  /** 当前语言（响应式）。 */
  locale: () => Locale;
  /** 切换语言。 */
  setLocale: (locale: Locale) => void;
  /** 翻译函数：`t("a.b.c", { param })` 或 `t("a.b.c")`。 */
  t: TFunction;
}

const I18nContext = createContext<I18nContextValue>();

const LOCALE_STORAGE_KEY = "vitrytool.locale";

function loadLocale(): Locale {
  try {
    const saved = localStorage.getItem(LOCALE_STORAGE_KEY);
    return saved === "zh-CN" || saved === "en-US" ? saved : "zh-CN";
  } catch {
    return "zh-CN";
  }
}

/** 轻量插值：将 `{name}` 替换为参数值；未提供的 key 保持原样。 */
function format(template: string, params?: Record<string, string | number | boolean>): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, key: string) =>
    key in params ? String(params[key]) : match,
  );
}

export function I18nProvider(props: ParentProps) {
  const [locale, setLocaleSignal] = createSignal<Locale>(loadLocale());

  // 跨窗口语言同步：主窗口与小窗（popup）各自持有独立的 I18nProvider 实例，
  // 任一窗口切换语言都会写入 localStorage，同源其他窗口经 storage 事件跟随（0.2.1 修复）。
  onMount(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === LOCALE_STORAGE_KEY && (e.newValue === "zh-CN" || e.newValue === "en-US")) {
        setLocaleSignal(e.newValue);
      }
    };
    window.addEventListener("storage", onStorage);
    onCleanup(() => window.removeEventListener("storage", onStorage));
  });

  const t: TFunction = (key, params) => {
    const dict = flatDictionaries[locale()] as Record<string, unknown>;
    const value = dict[key];
    return typeof value === "string" ? format(value, params) : String(value ?? "");
  };

  const setLocale = (next: Locale) => {
    setLocaleSignal(next);
    try {
      localStorage.setItem(LOCALE_STORAGE_KEY, next);
    } catch {
      // 忽略持久化失败
    }
  };

  const value: I18nContextValue = { locale, setLocale, t };
  return <I18nContext.Provider value={value}>{props.children}</I18nContext.Provider>;
}

export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error("useI18n 必须在 I18nProvider 内使用");
  return ctx;
}
