//! 设置页：语言、主题、剪贴板条数上限。
//!
//! - 语言 / 主题为纯前端设置，经 `useI18n` / `useTheme` 持久化到 localStorage。
//! - 条数上限走后端 `setMaxEntries`（后端逻辑需要 n，见契约）；**失焦即保存**，
//!   无效输入（越界/非整数）时恢复为已保存值，不弹保存按钮与成功提示。

import { createSignal, For, onMount } from "solid-js";
import { getErrorCode, getMaxEntries, setMaxEntries } from "../../api/clipboard-history";
import { locales, useI18n, type Locale } from "../../i18n";
import { useTheme, type Theme } from "../../theme";

const THEME_OPTIONS: { value: Theme; labelKey: string }[] = [
  { value: "light", labelKey: "settings.themeLight" },
  { value: "dark", labelKey: "settings.themeDark" },
  { value: "system", labelKey: "settings.themeSystem" },
];

const MAX_ENTRIES_MIN = 1;
const MAX_ENTRIES_MAX = 1024;

export function Settings() {
  const { t, locale, setLocale } = useI18n();
  const { theme, setTheme } = useTheme();
  const [maxInput, setMaxInput] = createSignal(64);
  const [savedMax, setSavedMax] = createSignal(64);
  const [error, setError] = createSignal("");

  onMount(() => {
    void getMaxEntries()
      .then((n) => {
        setMaxInput(n);
        setSavedMax(n);
      })
      .catch((err) => setError(t(getErrorCode(err) || "clipboard.storage_error")));
  });

  /** 失焦保存：有效则写后端，无效（越界/非整数）则恢复为已保存值。 */
  async function saveMaxOnBlur() {
    const n = Number(maxInput());
    if (!Number.isInteger(n) || n < MAX_ENTRIES_MIN || n > MAX_ENTRIES_MAX) {
      setMaxInput(savedMax());
      return;
    }
    if (n === savedMax()) return;
    try {
      const resp = await setMaxEntries(n);
      setMaxInput(resp.maxEntries);
      setSavedMax(resp.maxEntries);
      setError("");
    } catch (err) {
      setError(t(getErrorCode(err) || "clipboard.storage_error"));
    }
  }

  return (
    <section class="settings-page">
      <div class="settings-group">
        <div class="settings-group-title">{t("settings.language")}</div>
        <div class="settings-row">
          <span class="settings-label">{t("settings.language")}</span>
          <div class="segmented">
            <For each={locales}>
              {(l) => (
                <button
                  type="button"
                  class={locale() === l ? "active" : ""}
                  onClick={() => setLocale(l as Locale)}
                >
                  {l}
                </button>
              )}
            </For>
          </div>
        </div>
      </div>

      <div class="settings-group">
        <div class="settings-group-title">{t("settings.appearance")}</div>
        <div class="settings-row">
          <div>
            <div class="settings-label">{t("settings.theme")}</div>
            <div class="settings-desc">{t("settings.themeDesc")}</div>
          </div>
          <div class="segmented">
            <For each={THEME_OPTIONS}>
              {(o) => (
                <button
                  type="button"
                  class={theme() === o.value ? "active" : ""}
                  onClick={() => setTheme(o.value)}
                >
                  {t(o.labelKey)}
                </button>
              )}
            </For>
          </div>
        </div>
      </div>

      <div class="settings-group">
        <div class="settings-group-title">{t("settings.clipboard")}</div>
        <div class="settings-row">
          <div>
            <div class="settings-label">{t("settings.maxEntries")}</div>
            <div class="settings-desc">{t("settings.maxEntriesDesc")}</div>
          </div>
          <input
            class="number-input"
            type="number"
            min={MAX_ENTRIES_MIN}
            max={MAX_ENTRIES_MAX}
            value={maxInput()}
            onInput={(e) => setMaxInput(Number(e.currentTarget.value))}
            onBlur={() => void saveMaxOnBlur()}
          />
        </div>
        {error() && (
          <div class="settings-row">
            <span class="message error">{error()}</span>
          </div>
        )}
      </div>
    </section>
  );
}
