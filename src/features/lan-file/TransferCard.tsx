//! lan-file 传输卡（契约 `docs/api/lan-file.md` 5.3；设计决策 F4/F5）。
//!
//! 七态渲染：offering / transferring / resuming / done / failed / cancelled / rejected。
//! - transferring：实时进度条 + 速率（后端 EMA）+ 子文件进度；
//! - resuming：琥珀横幅「网络中断，等待恢复…」（不显重试次数）；
//! - done：材质化收束（发送侧淡出摘要 / 接收侧「打开所在位置」）；
//! - failed/cancelled/rejected：停留展示错误码（i18n）+ 重试按钮（重试 = 重新发起）。

import { createMemo, Show } from "solid-js";
import { formatBytes, formatSpeed, type LanFileTransfer } from "../../api/lan-file";
import { useI18n } from "../../i18n";

export function TransferCard(props: { transfer: LanFileTransfer }) {
  const { t } = useI18n();
  const tf = () => props.transfer;
  const percent = createMemo(() => {
    const v = tf();
    if (!v || v.totalBytes <= 0) return 0;
    return Math.min(100, Math.round((v.transferredBytes / v.totalBytes) * 100));
  });

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
          {tf().files.map((f) => (
            <li class={`transfer-file status-${f.status}`}>
              <span class="transfer-file-name">{f.name}</span>
              <span class="transfer-file-size">
                {f.status === "done" ? formatBytes(f.size) : `${formatBytes(f.transferredBytes)} / ${formatBytes(f.size)}`}
              </span>
            </li>
          ))}
        </ul>
      </Show>

      {/* 错误（failed/cancelled/rejected）：稳定错误码 i18n */}
      <Show when={tf().error}>
        <div class="transfer-error">
          {t(tf().error!.code in {} ? "notify.unknown" : tf().error!.code.replace("lan_file.", "lanFile."))}
        </div>
      </Show>

      {/* done（接收侧）：落盘路径 */}
      <Show when={tf().state === "done" && tf().savedPaths && tf().savedPaths!.length > 0}>
        <ul class="transfer-saved">
          {tf().savedPaths!.map((p) => (
            <li class="transfer-saved-path" title={p}>
              {p.split(/[\\/]/).pop()}
            </li>
          ))}
        </ul>
      </Show>
    </div>
  );
}
