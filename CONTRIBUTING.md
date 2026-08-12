# 贡献指南

感谢你对 VitryTool 的关注。本项目的核心原则：**接口契约先行、前后端分离、每个功能独立 mod、文档随代码同步**。请遵循以下流程与规范。

## 快速流程

1. **讨论**：新功能或改动先在 issue 中提出，明确需求与范围。
2. **接口契约**：在 `docs/api/` 下为功能规划接口契约文档（使用 `docs/api/TEMPLATE.md` 模板），明确请求/响应类型、错误码与变更影响。
3. **实现**：后端在 `src-tauri/src/features/<feature>/` 新建独立 mod 实现；前端在 `src/features/<feature>/` 对接，只通过 `invoke` 调用接口。
4. **测试**：后端功能必须有测试（单元测试 / doctest，覆盖度不做硬性要求）；前端使用 vitest。
5. **文档**：更新功能文档（`docs/features/`）、架构文档（如有影响）、`CHANGELOG.md` 与版本号（见 `docs/versioning.md`）。
6. **提交 PR**：提交说明描述改动与文档更新情况。

## 代码规范

- Rust：遵循 `rustfmt` 默认格式与 `cargo clippy` 建议；为公开项编写 rustdoc 注释。
- TypeScript / SolidJS：保持现有风格，类型严格。
- 文档与提交说明使用中文（代码注释、标识符、commit message 可中英混用，但保持一致）。

## 测试要求

- 后端：每个功能 mod 内必须有测试。新功能没有测试的 PR 不予合并。
- 前端：涉及 UI 逻辑的改动需补充/更新 vitest 用例。

## 文档要求

- **接口变更必须同步更新接口契约文档**，禁止「只改代码不改文档」。
- 新功能必须编写 `docs/features/<feature>.md` 详细文档。
- 版本变化按 `docs/versioning.md` 约定执行。

## 许可证授权

本项目以 MIT 许可证发布。提交贡献即表示你同意：你的贡献将以 MIT 许可证（见 `LICENSE`）授权给本项目，无需额外 CLA。
