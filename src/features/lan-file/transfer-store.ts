//! lan-file 传输快照的 App 级单例（契约 `docs/api/lan-file.md` 5.3）。
//!
//! 为什么放在组件外：`lan-file://transfer-updated` 是活跃任务的全量快照，
//! 若监听挂在 `FilePage` 内，用户切到别的页面期间事件会**丢失**，回到「文件」页
//! 又只在下一个事件到来时才显示——而 `offering`（等待对方确认）阶段没有后续事件，
//! 于是「点发送后切页面再回来，什么都没了」。此处在 App 挂载时启动一次全局监听，
//! 页面只消费信号。
//!
//! 同时保存「最近一次发起」的原始路径：失败卡片上的「加回待传列表」按钮需要它
//! （快照里只有文件名，没有本机绝对路径）。

import { createSignal } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import {
  LAN_FILE_TRANSFER_UPDATED_EVENT,
  type LanFileTransfer,
} from "../../api/lan-file";
import { isTerminalState } from "./TransferCard";

/** 最近一次传输快照（null = 无任务）。 */
const [transfer, setTransfer] = createSignal<LanFileTransfer | null>(null);

export { transfer };

/** 最近一次**发起**的原始入参（失败后可一键加回待传列表）。 */
const [lastSend, setLastSend] = createSignal<{ peerId: string; paths: string[] } | null>(null);

export { lastSend };

/** 记录最近一次发起（`sendLanFile` 成功后调用）。 */
export function rememberSend(peerId: string, paths: string[]): void {
  setLastSend({ peerId, paths });
}

/** 清空卡片（终态「关闭」按钮）。 */
export function clearTransfer(): void {
  setTransfer(null);
}

/** 写入快照（事件监听内部用；测试可直接构造终态）。 */
export function setTransferSnapshot(next: LanFileTransfer | null): void {
  setTransfer(next);
}

let started: Promise<() => void> | null = null;

/** 启动全局监听（幂等；App 挂载时调用一次）。 */
export function startTransferWatch(): void {
  if (started) return;
  started = listen<LanFileTransfer>(LAN_FILE_TRANSFER_UPDATED_EVENT, (e) => {
    const payload = e.payload ?? null;
    // 空闲快照（transferId 省略）不抹掉终态卡：终态由用户关闭或新任务替换
    if (!payload || !payload.transferId) {
      setTransfer((prev) => (prev && isTerminalState(prev.state) ? prev : null));
      return;
    }
    setTransfer(payload);
  });
}
