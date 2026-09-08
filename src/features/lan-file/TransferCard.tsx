//! lan-file 传输卡（契约 `docs/api/lan-file.md` 5.3；设计决策 F4/F5）。
//!
//! 七态渲染：offering / transferring / resuming / done / failed / cancelled / rejected。
//! - transferring：实时进度条 + 速率（后端 EMA）+ 子文件进度 + 「取消传输」；
//! - resuming：琥珀横幅「网络中断，等待恢复…」（不显重试次数）；
//! - done：接收侧列落盘文件 + 「打开所在位置」（发送侧为摘要）；
//! - failed/cancelled/rejected：停留展示错误码（i18n）+ 「重试」（发送侧重新发起）+ 「关闭」。
//!
//! 终态由后端 `lan-file://transfer-updated` 停留推送（不再紧跟 idle 抹掉卡片），
//! 因此本组件对终态提供显式关闭入口。

import { createMemo, For, Show } from "solid-js";
import {
  formatBytes,
  formatSpeed,
  translateLanFileError,
  type LanFileTransfer,
} from "../../api/lan-file";
import { useI18n } from "../../i18n";

/** 终态集合（卡片停留展示；可关闭 / 可重试）。 */
const TERMINAL_STATES = ["done", "failed", "cancelled", "rejected"] as const;

export function isTerminalState(state: LanFileTransfer["state"]): boolean {
  return (TERMINAL_STATES as readonly string[]).includes(state);
}

export interface TransferCardProps {
  transfer: LanFileTransfer;
  /** 取消进行中的传输（契约 5.5：显式取消 = 永久终止，不再续传）。 */
  onCancel?: (transferId: string) => void;
  /** 失败/取消后把该次传输的文件**加回待传列表**（不自动重发，由用户重新选终端）。 */
  onAddToPending?: (transfer: LanFileTransfer) => void;
  /** 关闭终态卡片（回到空闲展示）。 */
  onDismiss?: () => void;
  /** 打开落盘文件所在位置（接收侧 done）。 */
  onReveal?: (path: string) => void;
}

export function TransferCard(props: TransferCardProps) {
  const { t } = useI18n();
  const tf = () => props.transfer;
  const percent = createMemo(() => {
    const v = tf();
    if (!v || v.totalBytes <= 0) return 0;
    return Math.min(100, Math.round((v.transferredBytes / v.totalBytes) * 100));
  });

  const isActive = () =>
    tf().state === "offering" || tf().state === "transferring" || tf().state === "resuming";
  // 失败/取消/被拒的**发送**任务：提供「加回待传列表」（接收侧无本机路径，不提供）
  const canAddBack = () =>
    (tf().state === "failed" || tf().state === "cancelled" || tf().state === "rejected") &&
    tf().direction === "send" &&
    !!props.onAddToPending;
  const savedPaths = () => tf().savedPaths ?? [];
  const canReveal = () => tf().state === "done" && savedPaths().length > 0 && !!props.onReveal;

  const stateLabel = () => {
    const s = tf().state;
    switch (s) {
      case "offering":
        return t("lanFile.state.offering");
      case "transferring":
        return t("lanFile.state.transferring");
      case "resuming":
        return t("lanFile.state.resuming");
      case "done":
        return t("lanFile.state.done");
      case "failed":
        return t("lanFile.state.failed");
      case "cancelled":
        return t("lanFile.state.cancelled");
      case "rejected":
        return t("lanFile.state.rejected");
      default:
        return s;
    }
  };

  const directionLabel = () =>
    tf().direction === "send" ? t("lanFile.dirSend") : t("lanFile.dirReceive");

  const terminalName = () => tf().terminalName || tf().peerId?.slice(0, 12) || "";

  return (
    <div class={`transfer-card state-${tf().state}`} role="status">
      <div class="transfer-card-header">
        <span class="transfer-direction">{directionLabel()}</span>
        <span class="transfer-peer">{terminalName()}</span>
        <span class={`transfer-state state-${tf().state}`}>{stateLabel()}</span>
        {/* 终态：显式关闭（契约 F4：完成后收束 / 失败停留至用户处理） */}
        <Show when={isTerminalState(tf().state) && props.onDismiss}>
          <button
            type="button"
            class="transfer-dismiss"
            title={t("lanFile.dismiss")}
            aria-label={t("lanFile.dismiss")}
            onClick={() => props.onDismiss?.()}
          >
            ×
          </button>
        </Show>
      </div>

      {/* resuming 琥珀横幅（契约 F4：不显重试数） */}
      <Show when={tf().state === "resuming"}>
        <div class="transfer-resume-banner">{t("lanFile.resumingBanner")}</div>
      </Show>

      {/* 进度条（transferring/resuming） */}
      <Show when={tf().state === "transferring" || tf().state === "resuming"}>
        <div class="transfer-progress">
          <div class="transfer-progress-track">
            <div class="transfer-progress-fill" style={{ width: `${percent()}%` }} />
          </div>
          <div class="transfer-progress-meta">
            <span>
              {formatBytes(tf().transferredBytes)} / {formatBytes(tf().totalBytes)}（{percent()}%）
            </span>
            <Show when={tf().state === "transferring" && tf().bytesPerSec > 0}>
              <span class="transfer-speed">{formatSpeed(tf().bytesPerSec)}</span>
            </Show>
          </div>
        </div>
      </Show>

      {/* 子文件进度 */}
      <Show when={tf().files.length > 0}>
        <ul class="transfer-files">
          <For each={tf().files}>
            {(f) => (
              <li class={`transfer-file status-${f.status}`}>
                <span class="transfer-file-name">{f.name}</span>
                <span class="transfer-file-size">
                  {f.status === "done"
                    ? formatBytes(f.size)
                    : `${formatBytes(f.transferredBytes)} / ${formatBytes(f.size)}`}
                </span>
              </li>
            )}
          </For>
        </ul>
      </Show>

      {/* 错误（failed/cancelled/rejected）：稳定错误码 i18n */}
      <Show when={tf().error}>
        <div class="transfer-error">{translateLanFileError(t, tf().error!.code)}</div>
      </Show>

      {/* done（接收侧）：落盘文件 + 打开所在位置 */}
      <Show when={tf().state === "done" && savedPaths().length > 0}>
        <ul class="transfer-saved">
          <For each={savedPaths()}>
            {(p) => (
              <li class="transfer-saved-path" title={p}>
                {p.split(/[\\/]/).pop()}
              </li>
            )}
          </For>
        </ul>
      </Show>

      {/* 操作行：进行中可取消；终态可打开位置 / 加回待传列表 */}
      <Show when={(isActive() && props.onCancel) || canAddBack() || canReveal()}>
        <div class="transfer-actions">
          <Show when={isActive() && props.onCancel}>
            <button
              type="button"
              class="btn-ghost transfer-cancel"
              onClick={() => props.onCancel?.(tf().transferId ?? "")}
            >
              {t("lanFile.cancelTransfer")}
            </button>
          </Show>
          <Show when={canReveal()}>
            <button
              type="button"
              class="btn-ghost"
              onClick={() => props.onReveal?.(savedPaths()[0])}
            >
              {t("lanFile.openSaved")}
            </button>
          </Show>
          <Show when={canAddBack()}>
            <button
              type="button"
              class="btn-primary transfer-add-back"
              onClick={() => props.onAddToPending?.(tf())}
            >
              {t("lanFile.addToPending")}
            </button>
          </Show>
        </div>
      </Show>
    </div>
  );
}
