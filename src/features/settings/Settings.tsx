//! 设置页：语言、主题、剪贴板条数上限、快速粘贴快捷键。
//!
//! - 语言 / 主题为纯前端设置，经 `useI18n` / `useTheme` 持久化到 localStorage。
//! - 条数上限走后端 `setMaxEntries`（后端逻辑需要 n，见契约）；**失焦即保存**，
//!   无效输入（越界/非整数）时恢复为已保存值，不弹保存按钮与成功提示。
//! - 快捷键走 `HotkeyRecorder` + 后端 `setHotkey`（全局注册 + 持久化，契约见
//!   `docs/api/quick-paste.md`）；保存成功提示、失败回滚显示。

import { createSignal, For, onMount, Show } from "solid-js";
import {
  getErrorCode,
  getMaxEntries,
  setMaxEntries,
} from "../../api/clipboard-history";
import {
  getLanSyncStatus,
  setLanSyncBroadcast,
  setLanSyncReceive,
  setLanSyncTerminalName,
  type LanSyncStatus,
} from "../../api/lan-sync";
import { getHotkey, getHotkeyCapability, setHotkey } from "../../api/quick-paste";
import { locales, useI18n, type Locale } from "../../i18n";
import { useTheme, type Theme } from "../../theme";
import { HotkeyRecorder } from "../quick-paste/HotkeyRecorder";

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
  const [hotkey, setHotkeyValue] = createSignal("");
  const [error, setError] = createSignal("");
  const [notice, setNotice] = createSignal("");
  /** 全局快捷键能力检测：null=加载中；false=当前环境不支持（隐藏设置入口、显示警告）。 */
  const [hotkeySupported, setHotkeySupported] = createSignal<boolean | null>(null);
  /** 局域网同步状态（lan-sync，0.2.5）。 */
  const [lanStatus, setLanStatus] = createSignal<LanSyncStatus | null>(null);
  const [terminalInput, setTerminalInput] = createSignal("");
  const [savedTerminal, setSavedTerminal] = createSignal("");

  onMount(() => {
    // 检测失败按「支持」处理（fail-open，不打扰正常环境用户）
    void getHotkeyCapability()
      .then((c) => setHotkeySupported(c.supported))
      .catch(() => setHotkeySupported(true));

    void getMaxEntries()
      .then((n) => {
        setMaxInput(n);
        setSavedMax(n);
      })
      .catch((err) => setError(t(getErrorCode(err) || "clipboard.storage_error")));

    void getHotkey()
      .then((hk) => setHotkeyValue(hk ?? ""))
      .catch((err) => setError(t(getErrorCode(err) || "quickPaste.storage_error")));

    void getLanSyncStatus()
      .then((s) => {
        setLanStatus(s);
        setTerminalInput(s.terminalName);
        setSavedTerminal(s.terminalName);
      })
      .catch((err) => setError(t(getErrorCode(err) || "lanSync.peer_node_error")));
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

  /** 保存快捷键（含清除）：成功提示；失败回滚为已保存值。 */
  async function saveHotkey(next: string) {
    setNotice("");
    try {
      await setHotkey(next);
      setHotkeyValue(next);
      setError("");
      if (next) setNotice(t("quickPaste.saved"));
    } catch (err) {
      setError(t(getErrorCode(err) || "quickPaste.storage_error"));
      try {
        setHotkeyValue((await getHotkey()) ?? "");
      } catch {
        // 回滚读取失败则保持原显示
      }
    }
  }

  /** 开关切换：乐观更新，失败回滚并提示。 */
  async function toggleLan(key: "broadcast" | "receive", next: boolean) {
    const prev = lanStatus();
    if (!prev) return;
    setLanStatus({ ...prev, [key === "broadcast" ? "broadcastEnabled" : "receiveEnabled"]: next });
    try {
      if (key === "broadcast") await setLanSyncBroadcast(next);
      else await setLanSyncReceive(next);
      setError("");
    } catch (err) {
      if (prev) setLanStatus(prev);
      setError(t(getErrorCode(err) || "lanSync.peer_node_error"));
    }
  }

  /** 终端名失焦保存：有效则写后端；无效恢复为已保存值。 */
  async function saveTerminalOnBlur() {
    const name = terminalInput().trim();
    if (name.length === 0 || name.length > 32) {
      setTerminalInput(savedTerminal());
      return;
    }
    if (name === savedTerminal()) return;
    try {
      await setLanSyncTerminalName(name);
      setSavedTerminal(name);
      setLanStatus((s) => (s ? { ...s, terminalName: name } : s));
      setNotice(t("lanSync.saved"));
      setError("");
    } catch (err) {
      setError(t(getErrorCode(err) || "lanSync.invalid_name"));
      setTerminalInput(savedTerminal());
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
      </div>

      <div class="settings-group">
        <div class="settings-group-title">{t("lanSync.settingsTitle")}</div>
        <Show when={lanStatus()} fallback={<div class="settings-row"><span class="settings-label">{t("lanSync.nodeOffline")}</span></div>}>
          {(status) => (
            <>
              <div class="settings-row">
                <div>
                  <div class="settings-label">{t("lanSync.broadcast")}</div>
                  <div class="settings-desc">{t("lanSync.broadcastDesc")}</div>
                </div>
                <button
                  type="button"
                  role="switch"
                  aria-checked={status().broadcastEnabled}
                  class={status().broadcastEnabled ? "switch on" : "switch"}
                  onClick={() => void toggleLan("broadcast", !status().broadcastEnabled)}
                >
                  <span class="switch-knob" />
                </button>
              </div>
              <div class="settings-row">
                <div>
                  <div class="settings-label">{t("lanSync.receive")}</div>
                  <div class="settings-desc">{t("lanSync.receiveDesc")}</div>
                </div>
                <button
                  type="button"
                  role="switch"
                  aria-checked={status().receiveEnabled}
                  class={status().receiveEnabled ? "switch on" : "switch"}
                  onClick={() => void toggleLan("receive", !status().receiveEnabled)}
                >
                  <span class="switch-knob" />
                </button>
              </div>
              <div class="settings-row">
                <div>
                  <div class="settings-label">{t("lanSync.terminalName")}</div>
                  <div class="settings-desc">{t("lanSync.terminalNameDesc")}</div>
                </div>
                <input
                  class="number-input"
                  type="text"
                  maxlength={32}
                  value={terminalInput()}
                  onInput={(e) => setTerminalInput(e.currentTarget.value)}
                  onBlur={() => void saveTerminalOnBlur()}
                />
              </div>
              <div class="settings-row">
                <div>
                  <div class="settings-label">{t("lanSync.peersOnline", { count: status().peerCount })}</div>
                  <div class="settings-desc" title={status().peerId}>
                    {status().peerId.slice(0, 10)}…
                  </div>
                </div>
              </div>
            </>
          )}
        </Show>
      </div>

      <div class="settings-group">
        <div class="settings-group-title">{t("quickPaste.title")}</div>
        <Show
          when={hotkeySupported() !== false}
          fallback={
            <div class="settings-row">
              <div class="message warning">
                <div class="settings-label">{t("quickPaste.unsupportedTitle")}</div>
                <div class="settings-desc">{t("quickPaste.unsupportedDesc")}</div>
              </div>
            </div>
          }
        >
          <div class="settings-row">
            <div>
              <div class="settings-label">{t("quickPaste.hotkey")}</div>
              <div class="settings-desc">{t("quickPaste.hotkeyDesc")}</div>
            </div>
            <HotkeyRecorder value={hotkey()} onChange={(hk) => void saveHotkey(hk)} />
          </div>
        </Show>
      </div>

      {error() && (
        <div class="settings-row">
          <span class="message error">{error()}</span>
        </div>
      )}
      {notice() && (
        <div class="settings-row">
          <span class="message notice">{notice()}</span>
        </div>
      )}
    </section>
  );
}
