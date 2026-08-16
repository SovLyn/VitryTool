//! 品牌图标渲染脚本：原始 SVG 源 → 1024×1024 PNG。
//!
//! 唯一设计源：`src-tauri/icons/galaxy.svg`（保留原始文件，含 SVG Repo 的
//! 根级 `rotate(-45)` 变换）。resvg 对「根元素 rotate 绕原点」的处理会把图形
//! 旋出 viewBox 导致裁剪，故渲染前做一次等价的「居中旋转」修复：
//! 移除根 transform，将内容包进 `<g transform="rotate(-45 256 256)">`。
//! 其余图标产物（icon.ico / icon.icns / 各尺寸 png）均由 `pnpm tauri icon`
//! 从本脚本产出的 PNG 生成。
//!
//! 用法：`node scripts/render-icon.mjs`
//! 输出：`src-tauri/icons/app-icon.png`（1024×1024）

import { Resvg } from "@resvg/resvg-js";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const svgPath = join(root, "src-tauri", "icons", "galaxy.svg");
const outPath = join(root, "src-tauri", "icons", "app-icon.png");

const raw = readFileSync(svgPath, "utf8");

// SVG Repo 在根元素上的原始 transform（绕原点旋转，resvg 会裁剪）
const LEGACY_ROOT_TRANSFORM = ' transform="rotate(-45)matrix(1, 0, 0, 1, 0, 0)"';
// 根 svg 开标签（用于注入居中旋转组）
const SVG_OPEN = '<svg height="256px" width="256px" version="1.1" id="Layer_1" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 512 512" xml:space="preserve" fill="#000000">';

function toCenterRotated(svg) {
  if (!svg.includes(LEGACY_ROOT_TRANSFORM)) {
    throw new Error("galaxy.svg 结构变化：未找到根级 rotate(-45) transform，请检查渲染脚本");
  }
  const fixed = svg
    .replace(LEGACY_ROOT_TRANSFORM, "")
    .replace(SVG_OPEN, `${SVG_OPEN}<g transform="rotate(-45 256 256)">`)
    .replace("</svg>", "</g></svg>");
  return fixed;
}

const resvg = new Resvg(toCenterRotated(raw), {
  fitTo: { mode: "width", value: 1024 },
  background: "rgba(0,0,0,0)",
});
const png = resvg.render().asPng();
writeFileSync(outPath, png);
console.log(`rendered ${svgPath} -> ${outPath} (${png.length} bytes)`);
