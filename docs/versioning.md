# 版本变化约定

本项目遵循语义化版本（SemVer），当前版本 **`0.2.7`**。

## 规则

| 变化类型 | 版本操作 | 示例 |
| --- | --- | --- |
| 新功能签发 | minor 递增（0.x 阶段） | `0.0.1-alpha` → `0.1.0` |
| Bug 修复 / 文档 | patch 递增 | `0.1.0` → `0.1.1` |
| 破坏性接口变更 | major 递增（或 0.x 阶段 minor） | `0.1.0` → `1.0.0` |

> **注（实际实践）**：0.2.x 阶段的功能扩展走 patch（如 0.2.3 平台能力检测、0.2.4 剪贴板收藏、0.2.5 局域网同步 lan-sync 均为新功能但 patch 递增）——这些是既有功能域的扩展而非独立里程碑；是否 minor 由签发时判断，以 CHANGELOG 与契约文档的「版本影响」标注为准。

- 预发布阶段（版本带 `-alpha` / `-beta`）允许频繁迭代，正式对外声明稳定后进入 `1.0.0` 前的 SemVer 规则。
- **每次有新功能签发时，约定并递增版本**，同时更新 `CHANGELOG.md`。

## 同步位置（三处必须一致）

1. `src-tauri/Cargo.toml` → `version`
2. `src-tauri/tauri.conf.json` → `version`
3. `package.json` → `version`

Tauri 构建时会校验 `Cargo.toml` 与 `tauri.conf.json` 的版本一致性，修改时务必三处同步。

## 发布流程备忘

1. 递增版本（三处同步）。
2. 更新 `CHANGELOG.md`（未发布条目转为已发布）。
3. 更新 README 中的状态/版本信息。
4. 打 tag（如 `v0.1.0`）。

详细的内部操作记录见 `dev/versioning.md`（不对外）。
