//! lan-file 提议面板（0.3.0，契约 `docs/api/lan-file.md` 5.4；设计决策 F2/F5）。
//!
//! 顶部下滑玻璃面板 + 60s 倒计时环；TOFU 可展开指纹；nameClash 红色警告卡
//! （默认按钮位权移到「拒绝」）。60s 无人响应由后端自动拒绝（offer_timeout），
//! 前端倒计时环仅视觉同步。

import { listen } from "@tauri-apps/api/event";
import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import {
  acceptLanFile,
  formatBytes,
  LAN_FILE_INCOMING_EVENT,
  rejectLanFile,
  type LanFileOffer,
} from "../../api/lan-file";
import { notify } from "../../api/notify";
import { getErrorCode } from "../../api/clipboard-history";
import { useI18n } from "../../i18n";

const OFFER_WINDOW_MS = 60_000;

export function OfferPanel() {
  const { t } = useI18n();
  const [offer, setOffer] = createSignal<LanFileOffer | null>(null);
  const [remaining, setRemaining] = createSignal(OFFER_WINDOW_MS);
  const [showFingerprint, setShowFingerprint] = createSignal(false);
  const [deciding, setDeciding] = createSignal(false);

  onMount(() => {
    const unlisten = listen<LanFileOffer>(LAN_FILE_INCOMING_EVENT, (e) => {
      setOffer(e.payload);
      setRemaining(OFFER_WINDOW_MS);
      setShowFingerprint(false);
    });
    onCleanup(() => {
      void unlisten.then((fn) => fn());
    });
  });

  // 倒计时视觉同步（60s 窗口；后端为准）
  createEffect(() => {
    if (!offer()) return;
    const started = Date.now();
    const timer = setInterval(() => {
      const left = OFFER_WINDOW_MS - (Date.now() - started);
      if (left <= 0) {
        setRemaining(0);
        setOffer(null);
      } else {
        setRemaining(left);
      }
    }, 250);
    onCleanup(() => clearInterval(timer));
  });

  async function accept() {
    const o = offer();
    if (!o || deciding()) return;
    setDeciding(true);
    try {
      await acceptLanFile(o.transferId);
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lan_file.transfer_failed" });
    } finally {
      setDeciding(false);
      setOffer(null);
    }
  }

  async function reject() {
    const o = offer();
    if (!o || deciding()) return;
    setDeciding(true);
    try {
      await rejectLanFile(o.transferId);
    } catch (err) {
      await notify({ level: "error", code: getErrorCode(err) || "lan_file.transfer_failed" });
    } finally {
      setDeciding(false);
      setOffer(null);
    }
  }

  const ringPercent = () => Math.round((remaining() / OFFER_WINDOW_MS) * 100);
  const seconds = () => Math.ceil(remaining() / 1000);

  return (
    <Show when={offer()}>
      {(o) => (
        <div class="offer-panel" role="dialog" aria-label={t("lanFile.offerTitle")}>
          <div class="offer-card">
            <div class="offer-header">
              <span class="offer-title">{t("lanFile.offerTitle")}</span>
              {/* 60s 倒计时环 */}
              <div
                class="offer-countdown"
                style={{ "--ring": `${ringPercent()}%` }}
                title={t("lanFile.offerCountdown", { seconds: seconds() })}
              >
                {seconds()}
              </div>
            </div>

            <div class="offer-peer">
              <span class="offer-peer-name">{o().terminalName}</span>
              <span class="offer-peer-fp" title={o().fingerprint}>
                {o().fingerprint.slice(0, 22)}…
              </span>
            </div>

            {/* nameClash 红色警告卡（契约 5.4 复审修订 A） */}
            <Show when={o().nameClash}>
              <div class="offer-clash-warning" role="alert">
                {t("lanFile.nameClashWarning", { name: o().terminalName })}
              </div>
            </Show>

            {/* 文件清单 */}
            <ul class="offer-files">
              {o().files.map((f) => (
                <li class="offer-file">
                  <span class="offer-file-name">{f.name}</span>
                  <span class="offer-file-size">{formatBytes(f.size)}</span>
                </li>
              ))}
            </ul>
            <div class="offer-total">
              {t("lanFile.offerTotal", { count: o().files.length, size: formatBytes(o().totalBytes) })}
            </div>

            {/* TOFU 展开：指纹人工比对（fingerprint 为纯展示参考，契约 5.4） */}
            <button
              type="button"
              class="offer-fingerprint-toggle"
              onClick={() => setShowFingerprint(!showFingerprint())}
            >
              {showFingerprint() ? t("lanFile.hideFingerprint") : t("lanFile.showFingerprint")}
            </button>
            <Show when={showFingerprint()}>
              <code class="offer-fingerprint">{o().fingerprint}</code>
            </Show>

            {/* 按钮位：nameClash 时默认位权在「拒绝」（契约 5.4） */}
            <div class="offer-actions" classList={{ "clash": o().nameClash }}>
              <button
                type="button"
                class={o().nameClash ? "btn-ghost" : "btn-primary"}
                onClick={() => void accept()}
                disabled={deciding()}
              >
                {t("lanFile.accept")}
              </button>
              <button
                type="button"
                class={o().nameClash ? "btn-danger" : "btn-ghost"}
                onClick={() => void reject()}
                disabled={deciding()}
              >
                {t("lanFile.reject")}
              </button>
            </div>
          </div>
        </div>
      )}
    </Show>
  );
}
