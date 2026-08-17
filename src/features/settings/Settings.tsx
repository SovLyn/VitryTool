//! 设置页：语言、主题、剪贴板条数上限、快速粘贴快捷键、局域网同步、通知测试（DEV）。
//!
//! - 语言 / 主题为纯前端设置，经 `useI18n` / `useTheme` 持久化到 localStorage。
//! - 条数上限走后端 `setMaxEntries`（后端逻辑需要 n，见契约）；**失焦即保存**，
//!   无效输入（越界/非整数）时恢复为已保存值，不弹保存按钮与成功提示。
//! - 快捷键走 `HotkeyRecorder` + 后端 `setHotkey`（全局注册 + 持久化，契约见
//!   `docs/api/quick-paste.md`）；保存成功提示、失败回滚显示。
//! - 操作反馈（保存/开关/快捷键）经全局通知（`notify()`，契约 `docs/api/notify.md`）；
//!   仅初次加载失败保留内联错误态。
//! - 「通知测试」分组为开发者调试工具（`import.meta.env.DEV` 门控，发布构建不渲染）。

import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import {
  getErrorCode,
  getMaxEntries,
  setMaxEntries,
} from "../../api/clipboard-history";
import {
  getLanSyncStatus,
  LAN_SETTINGS_UPDATED_EVENT,
  setLanSyncBroadcast,
  setLanSyncReceive,
  setLanSyncTerminalName,
  type LanSyncStatus,
} from "../../api/lan-sync";
import { notify, UNKNOWN_NOTIFY_CODE, type NotifyLevel } from "../../api/notify";
import { getPlatformInfo } from "../../api/platform";
import { getHotkey, getHotkeyCapability, setHotkey } from "../../api/quick-paste";
import { locales, useI18n, type Locale } from "../../i18n";
import { useTheme, type Theme } from "../../theme";
import { HotkeyRecorder } from "../quick-paste/HotkeyRecorder";
import { listen } from "@tauri-apps/api/event";

const THEME_OPTIONS: { value: Theme; labelKey: string }[] = [
  { value: "light", labelKey: "settings.themeLight" },
  { value: "dark", labelKey: "settings.themeDark" },
  { value: "system", labelKey: "settings.themeSystem" },
];

const MAX_ENTRIES_MIN = 1;
const MAX_ENTRIES_MAX = 1024;

/** 通知测试组件可选的 level（契约 notify 5.3）。 */
const NOTIFY_LEVELS: NotifyLevel[] = ["success", "info", "warning", "error"];

/** 通知测试组件（DEV 门控，契约 notify 5.7）：自定义 level / code / params 走全链路。 */
function NotifyTester() {
  const { t } = useI18n();
  const [level, setLevel] = createSignal<NotifyLevel>("success");
  const [code, setCode] = createSignal("clipboard.copied");
  const [paramsText, setParamsText] = createSignal("");
  const [testerError, setTesterError] = createSignal("");

  async function send() {
    setTesterError("");
    let params: Record<string, string | number | boolean> | undefined;
    const raw = paramsText().trim();
    if (raw) {
      try {
        const parsed = JSON.parse(raw);
        if (parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)) {
          params = parsed as Record<string, string | number | boolean>;
        } else {
          setTesterError(t("notify.testParamsInvalid"));
          return;
        }
      } catch {
        setTesterError(t("notify.testParamsInvalid"));
        return;
      }
    }
    await notify({ level: level(), code: code().trim() || UNKNOWN_NOTIFY_CODE, params });
  }

  return (
    <div class="settings-group">
      <div class="settings-group-title">{t("notify.testTitle")}</div>
      <div class="settings-row">
        <span class="settings-label">{t("notify.testLevel")}</span>
        <div class="segmented">
          <For each={NOTIFY_LEVELS}>
            {(l) => (
              <button
                type="button"
                class={level() === l ? "active" : ""}
                onClick={() => setLevel(l)}
              >
                {l}
              </button>
            )}
          </For>
        </div>
      </div>
      <div class="settings-row">
        <div>
          <div class="settings-label">{t("notify.testCode")}</div>
          <div class="settings-desc">{t("notify.testDesc")}</div>
        </div>
        <input
          class="number-input notify-tester-code"
          type="text"
          value={code()}
          placeholder={t("notify.testCodePlaceholder")}
          onInput={(e) => setCode(e.currentTarget.value)}
        />
      </div>
      <div class="settings-row">
        <div>
          <div class="settings-label">{t("notify.testParams")}</div>
        </div>
        <input
          class="number-input notify-tester-params"
          type="text"
          value={paramsText()}
          placeholder={t("notify.testParamsPlaceholder")}
          onInput={(e) => setParamsText(e.currentTarget.value)}
        />
      </div>
      <div class="settings-row">
        <button type="button" class="btn-primary" onClick={() => void send()}>
          {t("notify.testSend")}
        </button>
        {testerError() && <span class="message error">{testerError()}</span>}
      </div>
    </div>
  );
}

export function Settings() {
  const { t, locale, setLocale } = useI18n();
  const { theme, setTheme } = useTheme();
  const [maxInput, setMaxInput] = createSignal(64);
  const [savedMax, setSavedMax] = createSignal(64);
  const [hotkey, setHotkeyValue] = createSignal("");
  /** 仅初次加载失败保留内联错误态（契约 notify 5.6）；操作反馈走全局通知。 */
  const [loadError, setLoadError] = createSignal("");
  /** 全局快捷键能力检测：null=加载中；false=当前环境不支持（隐藏设置入口、显示警告）。 */
  const [hotkeySupported, setHotkeySupported] = createSignal<boolean | null>(null);
  /** 是否移动端（null=加载中）：隐藏广播开关与快速粘贴组（契约 mobile 5.1）。 */
  const [isMobile, setIsMobile] = createSignal<boolean | null>(null);
  /** 局域网同步状态（lan-sync，0.2.5）。 */
  const [lanStatus, setLanStatus] = createSignal<LanSyncStatus | null>(null);
  const [terminalInput, setTerminalInput] = createSignal("");
  const [savedTerminal, setSavedTerminal] = createSignal("");

  onMount(() => {
    // 平台识别（契约 mobile 5.1）：移动端隐藏广播开关 / 快速粘贴组
    void getPlatformInfo()
      .then((info) => {
        setIsMobile(info.isMobile);
        // 桌面才需要快捷键能力与已存快捷键（移动端命令未注册，调用会报错）
        if (!info.isMobile) {
          void loadHotkeySettings();
        }
      })
      .catch(() => {
        setIsMobile(false); // 失败按桌面 fail-open
        void loadHotkeySettings();
      });

    void getMaxEntries()
      .then((n) => {
        setMaxInput(n);
        setSavedMax(n);
      })
      .catch((err) => setLoadError(t(getErrorCode(err) || "clipboard.storage_error")));

    refreshLanStatus();

    // 托盘快速开关（0.2.7）：后端切换广播/接收后 emit 设置变化事件，这里实时刷新开关状态
    const unlisten = listen(LAN_SETTINGS_UPDATED_EVENT, () => {
      void refreshLanStatus();
    });
    onCleanup(() => {
      void unlisten.then((fn) => fn());
    });
  });

  /** 加载快捷键能力检测与已存快捷键（桌面专属，移动端不调用）。 */
  function loadHotkeySettings() {
    // 检测失败按「支持」处理（fail-open，不打扰正常环境用户）
    void getHotkeyCapability()
      .then((c) => setHotkeySupported(c.supported))
      .catch(() => setHotkeySupported(true));

    void getHotkey()
      .then((hk) => setHotkeyValue(hk ?? ""))
      .catch((err) => setLoadError(t(getErrorCode(err) || "quickPaste.storage_error")));
  }

  /** 拉取 lan-sync 状态（初始加载 + 设置变化事件刷新）。 */
  async function refreshLanStatus() {
    try {
      const s = await getLanSyncStatus();
      setLanStatus(s);
      setTerminalInput((prev) => prev || s.terminalName);
      setSavedTerminal(s.terminalName);
    } catch (err) {
      setLoadError(t(getErrorCode(err) || "lanSync.peer_node_error"));
    }
  }

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
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "clipboard.storage_error" });
    }
  }

  /** 保存快捷键（含清除）：成功提示；失败回滚为已保存值。 */
  async function saveHotkey(next: string) {
    try {
      await setHotkey(next);
      setHotkeyValue(next);
      if (next) await notify({ level: "success", code: "quickPaste.saved" });
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "quickPaste.storage_error" });
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
    } catch (err) {
      if (prev) setLanStatus(prev);
      await notify({ level: "error", code: getErrorCode(err) || "lanSync.peer_node_error" });
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
      await notify({ level: "success", code: "lanSync.saved" });
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lanSync.invalid_name" });
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
              {/* 广播开关：桌面专属（移动端无广播实现，契约 mobile 5.1） */}
              <Show when={isMobile() === false}>
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
              </Show>
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

      {/* 快速粘贴：桌面专属（移动端无全局快捷键，契约 mobile 5.1），整组含标题隐藏 */}
      <Show when={isMobile() === false}>
        <div class="settings-group">
          <div class="settings-group-title">{t("quickPaste.title")}</div>
          <Show
            when={hotkeySupported() !== false}
            fallback={
              <Show when={hotkeySupported() === false}>
                <div class="settings-row">
                  <div class="message warning">
                    <div class="settings-label">{t("quickPaste.unsupportedTitle")}</div>
                    <div class="settings-desc">{t("quickPaste.unsupportedDesc")}</div>
                  </div>
                </div>
              </Show>
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
      </Show>

      {loadError() && (
        <div class="settings-row">
          <span class="message error">{loadError()}</span>
        </div>
      )}

      {/* 通知测试（契约 notify 5.7）：仅开发构建可见，验证通知全链路 */}
      {import.meta.env.DEV && <NotifyTester />}
    </section>
  );
}
