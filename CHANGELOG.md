# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 与语义化版本约定（见 `docs/versioning.md`）。

## [0.1.2] - 2026-08-13

### 新增

- **主题系统**：亮色 / 暗色 / 跟随系统，`src/theme.tsx`（localStorage 持久化 + `matchMedia` 实时跟随 + 首帧无闪烁）。
- **独立设置页**：语言、主题、剪贴板条数上限；语言/主题纯前端持久化。
- **左侧标签栏导航框架**：功能在上、设置固定在底部，为后续功能预留。
- **Apple 风格视觉系统**：亮暗两套语义色变量 + 玻璃材质（backdrop-filter）贯穿全局 + 即时按压反馈 + 排版层级。

### 变更

- 语言设置持久化到 localStorage（此前切语言不记忆）。
- 剪贴板条数上限改为**失焦即存**（移除保存按钮与成功提示；无效输入恢复原值）。
- 移除主界面「最多保留 N 条」提示。
- 设置由弹窗改为独立页面。

## [0.1.1] - 2026-08-13

### 新增

- **日志系统**：基于 `log` + `tauri-plugin-log`（`core/log.rs`）。开发构建输出 Trace 级到终端（系统 crate 压到 Info）；发布构建保存 Error 级到应用日志目录文件（5 MiB 轮转、保留最近一份）。开发构建同时将 Trace 落盘（便于排查）。
- 剪贴板历史各命令补齐日志（元数据为主，遵循隐私约束，不记录剪贴板明文）。

### 修复

- **孤儿清理误删图片**：`image_dir` 用单个含 `/` 的相对串 `join`，在 Windows 上保留正斜杠，与插件落盘路径（`\`）不一致，导致 `orphan_files` 字符串比较失败、全部图片被误判为孤儿删除。现改为分开 `join`，且路径比较改用 `Path::components`（`/` 与 `\` 视为同一分隔符），并加回归测试。
- **去重丢弃的已落盘图片成为孤儿**：`read_image` 提前落盘，但去重命中旧条目时新图可能不被采纳；现于 `capture_clipboard` 内清理本次落盘且未被任何条目引用的图片。

### 变更

- `docs/architecture.md` 新增「日志约定」章节。

## [0.1.0] - 2026-08-13

### 新增

- **首个功能：剪贴板历史（clipboard-history）**，契约见 `docs/api/clipboard-history.md`：
  - 全格式捕捉：文本 / HTML / RTF / 图片 / 文件引用，原始格式保真记录，时间戳必记。
  - 内容指纹去重置顶；数量上限可设置（默认 64，最大 1024），超限即时淘汰。
  - 图片由 tauri-plugin-clipboard-x 落盘于插件默认目录；前台定时（5 分钟）兜底清理孤儿图片。
  - 点击条目按原始格式回写剪贴板；支持单条删除与清空全部。
  - 启动即自动监听；图片缺失条目保留并标记。
- 架构决策记录：`docs/adr/0001-clipboard-capture-via-webview-events.md`（捕捉链路经前端事件驱动）。
- 领域术语表：`dev/CONTEXT.md`（内部文档，不对外发布）。

### 变更

- 移除脚手架 `greet` 命令与前端演示。
- 依赖新增：`tauri-plugin-clipboard-x`、`tauri-plugin-store`（Rust）、`tauri-plugin-clipboard-x-api`（前端）。
- 前端主界面由演示页切换为剪贴板历史页面。
- 修复图片预览无法加载：启用 `security.assetProtocol`（scope 限定图片目录），`<img>` 加载失败回退占位文案。

### 已知限制（后续迭代）

- 大列表（接近 1024 条富文本）时 store 全量序列化存在性能开销。
- 快速连续复制时，Windows 剪贴板监视延迟可能丢失中间内容。

## [0.0.1-alpha] - 2026-08-12

### 新增

- 项目框架骨架：Tauri 2 + SolidJS + TypeScript + Vite。
- 开源基础设施：MIT 许可证、README、贡献指南（CONTRIBUTING）、安全政策（SECURITY）。
- 文档体系：公开文档 `docs/`（架构、接口契约规范、功能文档指南、版本约定）与内部启发式文档 `dev/`（不对外发布）。
- 后端结构：按功能域划分 mod 的骨架（`core/` + `features/`）。
- 前端 i18n 基建（中文 / 英文）与 vitest 测试基建。

### 待办

- 首个功能（局域网信息共享）规划中，见 `docs/features/` 与 `docs/api/`。
- CI（GitHub Actions）与品牌图标：首个功能签发后接入。
