# VitryTool 品牌视觉规范

> 图标设计决策记录（0.2.6 起）。SVG 源文件是唯一设计源，其余图标产物均由脚本 + Tauri CLI 生成，**不要手工编辑** PNG/ICO/ICNS。

## Logo 说明

- **图形**：galaxy 图标（蓝色轨道环 + 中心球，来自 [SVG Repo](https://www.svgrepo.com)），象征「局域网中信息沿轨道流转、多终端互联」。
- **旋转**：原始 SVG 在根元素带 `rotate(-45)` 变换（SVG Repo Mixer 处理）；resvg 渲染根级旋转会裁剪，渲染脚本 `scripts/render-icon.mjs` 已做等价「居中旋转」修复（`<g transform="rotate(-45 256 256)">`）。
- **背景**：透明（无圆角方块底），依赖桌面壳/任务栏呈现；如需品牌容器色再评估。

## 配色（galaxy.svg 自带）

| 用途 | 色值 | 说明 |
| --- | --- | --- |
| 轨道环 | `#4274D9` | 主蓝 |
| 外环/中心外圈 | `#4B5694` | 深蓝紫 |
| 中心球 | `#4274D9` | 主蓝 |

## 源文件与生成流程

**唯一设计源**：`src-tauri/icons/galaxy.svg`（保留原始文件，可编辑、可评审 diff）。

生成流程（两步，全部可复现）：

```bash
# 1) SVG 源 → 1024×1024 PNG（脚本保留在仓库，含居中旋转修复）
node scripts/render-icon.mjs

# 2) PNG → 全套图标（icon.ico / icon.icns / 各尺寸 png / StoreLogo 等）
pnpm tauri icon src-tauri/icons/app-icon.png
```

产物（`src-tauri/icons/`，均由生成命令产出，勿手改）：
- `icon.ico`（Windows）、`icon.icns`（macOS）、`icon.png`（通用 512）
- `32x32.png` / `64x64.png` / `128x128.png` / `128x128@2x.png`（托盘与窗口）
- `Square*Logo.png` / `StoreLogo.png`（Windows Store 预留）
- `app-icon.png`（渲染中间产物，可随时由脚本重建）

## 图标应用位置

- **托盘图标**：`src-tauri/src/core/tray.rs`（`app.default_window_icon()`，随构建自动使用新图标）。
- **应用窗口图标**：打包时由 `tauri.conf.json` `bundle.icon` 引用。

## 变更记录

- **0.2.6 初版**：自绘「V 字 + 节点网络」（System.Drawing 绘制，已废弃，源文件已删除）。
- **0.2.6 修订 1-7**：多轮自绘迭代（V 字缩小、细线、四向/三向折线箭头、衬线 V、实心填充），均废弃。
- **0.2.6 终版**：改用用户提供的 galaxy.svg（SVG Repo 资源），保留原始 SVG 为唯一源。
