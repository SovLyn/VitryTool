# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 与语义化版本约定（见 `docs/versioning.md`）。

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
