//! 剪贴板历史本地增量更新（纯函数，无副作用）。
//!
//! 背景：本批优化前，每次复制（`clipboard-history://updated` 事件）都触发主窗口
//! 全量重新拉取（后端 90-300ms + 全量 JSON 过桥 + 整棵 DOM 重建）。现改为：
//! 捕捉成功后的事件载荷携带**完整新条目**（`listener.ts` emit），本模块在本地应用——
//! 插入/置顶 + 按缓存上限镜像淘汰 + 展示序排序（与后端 `service::sort_for_display`
//! 一致，契约 5.8），消除复制时的后端往返。收藏切换 / 删除 / 清空仍走全量刷新（低频）。
//!
//! 入参列表已按展示序（`getClipboardHistory` 返回序），本模块不改写入参。

import type { ClipboardEntry } from "../../api/clipboard-history";

/** 展示序比较器（与后端 `service::sort_for_display` 一致，契约 5.8）：
 * 收藏区在前（区内 `favoritedAt` 倒序，最近收藏最前），其后普通条目按 `capturedAt` 倒序。
 * ISO 8601 UTC 字符串可字典序比较 = 时间序。 */
export function compareForDisplay(a: ClipboardEntry, b: ClipboardEntry): number {
  const aFav = a.favoritedAt !== undefined;
  const bFav = b.favoritedAt !== undefined;
  if (aFav !== bFav) return aFav ? -1 : 1;
  if (aFav) return (b.favoritedAt ?? "").localeCompare(a.favoritedAt ?? "");
  return b.capturedAt.localeCompare(a.capturedAt);
}

/** 本地增量应用一次捕捉结果（返回新列表，不修改入参）。
 *
 * - 按 id 命中（去重置顶）：替换为最新内容后参与排序——后端已保证收藏状态与
 *   `favoritedAt` 不变、仅刷新 `capturedAt`，此处照单全收即可；
 * - 未命中：作为新条目插入；
 * - **镜像淘汰**：非收藏条目数 > `maxEntries` 时移除最旧的非收藏条目（收藏豁免，
 *   与后端 `evict_over_limit` 语义一致）——否则列表会在满上限时随每次复制无限增长；
 * - 最后按展示序排序（收藏区在前）。
 */
export function applyCapturedEntry(
  entries: readonly ClipboardEntry[],
  incoming: ClipboardEntry,
  maxEntries: number,
): ClipboardEntry[] {
  const next = entries.filter((e) => e.id !== incoming.id);
  next.unshift(incoming);
  next.sort(compareForDisplay);

  // 展示序下普通区按 capturedAt 倒序，队尾即最旧；从后向前淘汰最旧非收藏（收藏豁免）
  let over = next.reduce((n, e) => n + (e.favoritedAt === undefined ? 1 : 0), 0) - maxEntries;
  if (over > 0) {
    for (let i = next.length - 1; i >= 0 && over > 0; i--) {
      if (next[i].favoritedAt === undefined) {
        next.splice(i, 1);
        over -= 1;
      }
    }
  }
  return next;
}
