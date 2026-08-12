# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 与语义化版本约定（见 `docs/versioning.md`）。

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
