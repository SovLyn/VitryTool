//! Android 启动图标渲染脚本：从唯一设计源 `src-tauri/icons/galaxy.svg` 生成
//! `gen/android` 各密度 mipmap 位图（ic_launcher.png / ic_launcher_round.png）。
//!
//! 与 `render-icon.mjs` 同一套「居中旋转修复」逻辑（resvg 对根级 rotate(-45)
//! 绕原点旋转会旋出 viewBox 导致裁剪）。
//!
//! Android mipmap 密度尺寸：mdpi 48 / hdpi 72 / xhdpi 96 / xxhdpi 144 / xxxhdpi 192。
//! 用法：`node scripts/render-android-icons.mjs`
//! 产物：gen/android/app/src/main/res/mipmap-*/ic_launcher{,_round}.png

import { Resvg } from "@resvg/resvg-js";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const svgPath = join(root, "src-tauri", "icons", "galaxy.svg");
const resDir = join(root, "src-tauri", "gen", "android", "app", "src", "main", "res");

const raw = readFileSync(svgPath, "utf8");

// 与 render-icon.mjs 相同的源结构修复（见该脚本头部注释）
const LEGACY_ROOT_TRANSFORM = ' transform="rotate(-45)matrix(1, 0, 0, 1, 0, 0)"';
const SVG_OPEN = '<svg height="256px" width="256px" version="1.1" id="Layer_1" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 512 512" xml:space="preserve" fill="#000000">';

function toCenterRotated(svg) {
  if (!svg.includes(LEGACY_ROOT_TRANSFORM)) {
    throw new Error("galaxy.svg 结构变化：未找到根级 rotate(-45) transform，请检查渲染脚本");
  }
  return svg
    .replace(LEGACY_ROOT_TRANSFORM, "")
    .replace(SVG_OPEN, `${SVG_OPEN}<g transform="rotate(-45 256 256)">`)
    .replace("</svg>", "</g></svg>");
}

const fixed = toCenterRotated(raw);

// 密度 → (mipmap 目录名, 位图尺寸)
const DENSITIES = [
  ["mipmap-mdpi", 48],
  ["mipmap-hdpi", 72],
  ["mipmap-xhdpi", 96],
  ["mipmap-xxhdpi", 144],
  ["mipmap-xxxhdpi", 192],
];

for (const [dirName, size] of DENSITIES) {
  const dir = join(resDir, dirName);
  mkdirSync(dir, { recursive: true });
  const resvg = new Resvg(fixed, {
    fitTo: { mode: "width", value: size },
    background: "rgba(0,0,0,0)",
  });
  const png = resvg.render().asPng();
  for (const name of ["ic_launcher.png", "ic_launcher_round.png"]) {
    writeFileSync(join(dir, name), png);
  }
  console.log(`${dirName}: ${size}x${size} -> ic_launcher.png + ic_launcher_round.png`);
}
console.log("ANDROID_ICONS_DONE");
