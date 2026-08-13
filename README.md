# VitryTool

局域网信息共享工具（LAN information sharing）—— 用于在局域网内共享信息与文件的开源桌面应用。

> **当前状态**：`0.1.0`，首个功能（剪贴板历史）已落地。

## 项目简介

VitryTool 的目标是提供轻量、本地的局域网信息共享能力，不依赖云端服务，所有数据在局域网内流转。

当前仓库为**框架骨架**：前后端分离架构、接口契约流程、文档体系与工程规范已就位，具体功能在 `docs/features/` 中规划推进。

## 功能规划

- **已落地**：剪贴板历史（clipboard-history）——捕捉剪贴板变化并保存为可浏览、可回写的历史记录，见 [`docs/features/clipboard-history.md`](docs/features/clipboard-history.md) 与接口契约 [`docs/api/clipboard-history.md`](docs/api/clipboard-history.md)。
- **规划中**：局域网信息共享（首个功能原规划），接口契约文档先行，见 `docs/api/`。
- 功能进度与版本变化记录在 `CHANGELOG.md`。

## 技术栈

- **桌面壳**：Tauri 2（Rust）
- **前端**：SolidJS + TypeScript + Vite，内置 i18n（开发阶段支持中文 / 英文）
- **后端**：Rust，按功能域划分为独立 mod（`src-tauri/src/features/`）

## 架构

- **前后端分离**：前端 `src/` 与后端 `src-tauri/` 物理隔离，仅通过 Tauri command（`invoke`）通信；接口需先以契约文档形式规划，见 `docs/api/`。
- **后端按功能分 mod**：每个功能一个独立 mod，自包含命令、业务逻辑与测试；`lib.rs` 仅做组装。
- 详细说明见 [`docs/architecture.md`](docs/architecture.md)。

## 开发

环境要求：Rust（stable）、Node.js、pnpm（依赖管理；亦可使用 deno 执行脚本）。

```bash
pnpm install          # 安装前端依赖
pnpm tauri dev        # 启动开发（前后端一体，含热更新）
```

独立命令：

```bash
pnpm dev              # 仅前端（Vite dev server）
pnpm build            # 前端构建
pnpm tauri build      # 桌面端打包
```

## 测试

- 后端：`cargo test`（位于 `src-tauri/`，每个功能必须有**开发者测试（dt）**——随功能编写的单元测试 / doctest；覆盖度不做硬性要求）。
- 前端：`pnpm test`（vitest）。

## 文档

| 文档 | 说明 |
| --- | --- |
| [`docs/`](docs/README.md) | 文档总索引 |
| [`docs/architecture.md`](docs/architecture.md) | 架构与代码组织规范 |
| [`docs/api/`](docs/api/README.md) | 前后端接口契约规范与模板 |
| [`docs/features/`](docs/features/README.md) | 功能文档 |
| [`docs/versioning.md`](docs/versioning.md) | 版本变化约定 |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | 贡献指南 |
| [`SECURITY.md`](SECURITY.md) | 安全漏洞报告 |
| [`CHANGELOG.md`](CHANGELOG.md) | 变更日志 |

## 已知限制

- **剪贴板历史性能**：历史列表通过 tauri-plugin-store 整体序列化持久化，接近上限（1024 条富文本）时读写存在性能开销，后续版本优化。
- **连续复制**：Windows 剪贴板监视存在固有延迟，极速连续复制可能丢失中间内容。
- 剪贴板历史**明文存储**敏感内容（密码等），提供单条删除与清空全部；暂停监听功能规划中。

## 版本管理

当前版本 `0.1.0`。每次有新功能签发时按约定递增版本，规则见 [`docs/versioning.md`](docs/versioning.md)。

## 隐私

**纯本地运行**：无遥测、无统计数据上报、不访问任何云端服务。局域网内的数据仅在你控制的设备间流转。

## 贡献

欢迎参与。请先阅读 [`CONTRIBUTING.md`](CONTRIBUTING.md)：功能开发遵循「接口契约 → 后端实现 → 前端对接 → 文档更新」的流程。

## 安全

发现安全问题请直接提交 [issue](https://github.com/)（优先选择），见 [`SECURITY.md`](SECURITY.md)。

## 许可证

[MIT](LICENSE) © SovLyn
