import { describe, it, expect } from "vitest";
import zhCN from "./locales/zh-CN.json";
import enUS from "./locales/en-US.json";

/** 递归收集嵌套对象的点分 key。 */
function keys(obj: unknown, prefix = ""): string[] {
  if (typeof obj !== "object" || obj === null) return [prefix];
  return Object.entries(obj as Record<string, unknown>).flatMap(([k, v]) =>
    keys(v, prefix ? `${prefix}.${k}` : k),
  );
}

describe("i18n locales", () => {
  it("zh-CN 与 en-US 的 key 集合一致（新增文案必须双语同步）", () => {
    const zh = keys(zhCN).sort();
    const en = keys(enUS).sort();
    expect(zh).toEqual(en);
  });
});
