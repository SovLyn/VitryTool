# 文档总索引

本目录存放 **公开文档**（随仓库对外发布）。内部启发式文档（规划草稿、接口草案、探讨记录）位于 `dev/`，不纳入版本控制，不对外展示。

## 文档体系

| 文档 | 内容 | 维护时机 |
| --- | --- | --- |
| [architecture.md](architecture.md) | 架构与代码组织规范：前后端分离、后端 mod 划分、前端目录、i18n、测试策略 | 架构调整时 |
| [api/README.md](api/README.md) | 前后端接口契约规范：接口规划仪式、命名、错误码、变更流程 | 新增/修改接口时 |
| [api/TEMPLATE.md](api/TEMPLATE.md) | 新功能接口契约文档模板 | 每次功能探讨前置 |
| [features/README.md](features/README.md) | 功能文档指南：每个功能一份详细文档 | 新功能签发时 |
| [design/brand.md](design/brand.md) | 品牌视觉规范：Logo 语义 / 配色 / SVG 源与生成流程 / 图标应用位置 | 品牌或图标变更时 |
| [versioning.md](versioning.md) | 版本变化约定 | 版本递增时 |

## 阅读顺序建议

- 新贡献者：`README.md`（仓库根）→ `architecture.md` → 任一功能文档。
- 开始新功能：`api/TEMPLATE.md` → `features/README.md`（先契约后实现）。

## 相关文件

- 贡献流程：`CONTRIBUTING.md`（仓库根）
- 变更记录：`CHANGELOG.md`（仓库根）
- 内部文档：`dev/`（不对外）
