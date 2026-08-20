import { describe, expect, it } from "vitest";
import type { ClipboardEntry } from "../../api/clipboard-history";
import { applyCapturedEntry, compareForDisplay } from "./incremental";

function entry(id: string, capturedAt: string, favoritedAt?: string): ClipboardEntry {
  return { id, capturedAt, ...(favoritedAt ? { favoritedAt } : {}) };
}

describe("compareForDisplay（与后端 sort_for_display 一致，契约 5.8）", () => {
  it("收藏区在前，区内按 favoritedAt 倒序", () => {
    const list = [entry("a", "2026-08-13T00:00:00Z"), entry("b", "2026-08-13T02:00:00Z", "2026-08-13T01:00:00Z"), entry("c", "2026-08-13T03:00:00Z", "2026-08-13T02:00:00Z")];
    list.sort(compareForDisplay);
    expect(list.map((e) => e.id)).toEqual(["c", "b", "a"]);
  });

  it("普通区按 capturedAt 倒序", () => {
    const list = [entry("old", "2026-08-13T00:00:00Z"), entry("new", "2026-08-13T01:00:00Z")];
    list.sort(compareForDisplay);
    expect(list.map((e) => e.id)).toEqual(["new", "old"]);
  });
});

describe("applyCapturedEntry（本地增量应用）", () => {
  it("新条目插入普通区顶部，不修改入参", () => {
    const input = [entry("newer", "2026-08-13T02:00:00Z"), entry("older", "2026-08-13T01:00:00Z")];
    const incoming = entry("fresh", "2026-08-13T03:00:00Z");
    const result = applyCapturedEntry(input, incoming, 64);
    expect(result.map((e) => e.id)).toEqual(["fresh", "newer", "older"]);
    expect(input).toHaveLength(2); // 入参不被修改
    expect(input[0].id).toBe("newer");
  });

  it("按 id 命中（去重置顶）：替换并置顶，不产生重复", () => {
    const input = [entry("dup", "2026-08-13T02:00:00Z"), entry("other", "2026-08-13T01:00:00Z")];
    const promoted = entry("dup", "2026-08-13T03:00:00Z"); // 后端返回的既有条目（capturedAt 已刷新）
    const result = applyCapturedEntry(input, promoted, 64);
    expect(result).toHaveLength(2);
    expect(result[0].id).toBe("dup");
    expect(result[0].capturedAt).toBe("2026-08-13T03:00:00Z");
  });

  it("去重置顶的收藏条目留在收藏区（favoritedAt 不变）", () => {
    const fav = entry("fav", "2026-08-13T00:00:00Z", "2026-08-13T01:00:00Z");
    const input = [fav, entry("plain", "2026-08-13T02:00:00Z")];
    const promoted = entry("fav", "2026-08-13T03:00:00Z", "2026-08-13T01:00:00Z");
    const result = applyCapturedEntry(input, promoted, 64);
    expect(result.map((e) => e.id)).toEqual(["fav", "plain"]); // 收藏仍在前
  });

  it("镜像淘汰：非收藏超上限时移除最旧非收藏，收藏豁免", () => {
    const input = [
      entry("fav", "2026-08-13T00:00:00Z", "2026-08-13T01:00:00Z"),
      entry("r3", "2026-08-13T03:00:00Z"),
      entry("r2", "2026-08-13T02:00:00Z"),
      entry("r1", "2026-08-13T01:00:00Z"),
    ];
    const result = applyCapturedEntry(input, entry("r4", "2026-08-13T04:00:00Z"), 3);
    // 非收藏 4 > 上限 3 → 淘汰最旧（r1）；收藏条目豁免
    expect(result.map((e) => e.id)).toEqual(["fav", "r4", "r3", "r2"]);
  });

  it("多次超限连续淘汰直至非收藏不超上限", () => {
    const input = [entry("r3", "2026-08-13T03:00:00Z"), entry("r2", "2026-08-13T02:00:00Z"), entry("r1", "2026-08-13T01:00:00Z")];
    const result = applyCapturedEntry(input, entry("r4", "2026-08-13T04:00:00Z"), 2);
    expect(result.map((e) => e.id)).toEqual(["r4", "r3"]);
  });

  it("全收藏时不淘汰（收藏豁免，永不超限）", () => {
    const input = [entry("f1", "2026-08-13T01:00:00Z", "2026-08-13T01:00:00Z"), entry("f2", "2026-08-13T02:00:00Z", "2026-08-13T02:00:00Z")];
    const result = applyCapturedEntry(input, entry("f3", "2026-08-13T03:00:00Z", "2026-08-13T03:00:00Z"), 1);
    expect(result).toHaveLength(3);
  });
});
