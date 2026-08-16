# 接口契约文档：notify（全局通知系统）

- 状态：`已实现`（0.2.8）
- 关联功能文档：[docs/features/notify.md](../features/notify.md)
- 版本影响：`patch`（0.2.7 → 0.2.8；与近期功能实践一致，见 `docs/versioning.md` 注）
- 设计决策来源：2026-08 /grill-me 前后端商讨（后端 Q1–Q6，前端 F1–F11，测试组件 T1）

## 1. 概述

全局通知系统：**双向统一通道**——前端经 `notify` 命令提交、后端校验后广播 `app://notify` 事件到所有窗口；后端内部站点（托盘开关失败、快捷键注册失败、lan-sync 节点错误等）直接 emit 同一事件。前端由 NotificationProvider（仅主窗口挂载）监听并渲染为右上角 toast 堆栈，替代各页面原先的内联成功/错误消息。

- **负载为结构化 `level + code + params`，全链路不出现原始文案**：`code` 是稳定 i18n 键或后端错误码，前端在渲染时翻译（切语言即时重译，这是迁移的附带收益）。
- **后端不分配 id、不节流、不持久化**：去重/折叠/关闭是前端 toast 的职责；5 个后端站点均为用户一次性动作，无轰炸风险。
- 快速粘贴小屏（popup）**不渲染**通知（瞬态窗口，简略为要），其内联错误保持现状。

## 2. 命令列表

| 命令 | 方向 | 说明 |
| --- | --- | --- |
| `notify(level, code, params?)` | 前端 → 后端 | 提交通知；校验 level ∈ {success, error, warning, info} 且 code 非空，通过后广播 `app://notify` |

事件（后端 → 前端）：

| 事件 | 载荷 | 说明 |
| --- | --- | --- |
| `app://notify` | `{ level, code, params? }` | 通知广播（后端命令转发或内部站点直发；`app.emit` 广播到所有 WebView 窗口） |

## 3. 类型定义

### 请求 / 事件载荷（前后端同构）

```ts
// TypeScript（前端视角）
type NotifyLevel = "success" | "error" | "warning" | "info";

interface NotifyPayload {
  level: NotifyLevel;   // 级别（前端渲染语义见 5.3）
  code: string;         // 稳定 i18n 键（如 "clipboard.copied"）或后端错误码（如 "lan.peer_node_error"）
  params?: Record<string, string | number | boolean>; // 插值参数（可选）
}
```

```rust
// Rust（后端视角，serde）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NotifyLevel { Success, Error, Warning, Info } // 序列化为小写字符串

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyPayload {
    pub level: NotifyLevel,
    pub code: String,
    pub params: Option<serde_json::Map<String, serde_json::Value>>,
}
```

命令参数：`notify(level: String, code: String, params: Option<Map>)`——`level` 以字符串接收，后端校验（与「非法返回 `notify.invalid`」一致，而非 serde 反序列化报错）。

## 4. 错误码

| 错误码 | 含义 | 中文文案建议 | 英文文案建议 |
| --- | --- | --- | --- |
| `notify.invalid` | level 非法或 code 为空/纯空白 | （前端 fire-and-forget，一般不展示；兜底走 `notify.unknown`） | （同上） |

## 5. 行为说明

### 5.1 通道与数据流

- **前端 → 后端**：前端调 `notify()`（封装在 `src/api/notify.ts`，调用 `invoke("notify", ...)`）→ 后端校验 → `app.emit("app://notify", payload)` 广播到所有窗口 → 主窗口 NotificationProvider 渲染。前端调用为 fire-and-forget，`notify` 命令返回 `Result<(), ApiError>` 仅作异常兜底（前端 catch 后仅记日志，不再次通知——通知不该拖垮主流程）。
- **后端 → 前端**：内部站点直接调用 `core::notify::notify_app(&app, level, code)` → emit 同一事件。**只 emit 不阻塞**：emit 失败仅记日志（`let _ = app.emit(...)` 风格，与 lan-sync 既有 emit 一致），通知失败不影响主流程。

### 5.2 后端站点（仅此 5 个，其余 43 处日志不升级）

| 站点 | 场景 | level | code |
| --- | --- | --- | --- |
| `quick_paste::set_hotkey` 注册失败 | 用户刚配置的快捷键注册失败（可能被占用） | error | `quick_paste.register_failed` |
| 托盘「剪贴板广播」开关失败 | 用户点了托盘项没反应 | error | `quick_paste.tray_update_failed` |
| 托盘「剪贴板接收」开关失败 | 同上 | error | `quick_paste.tray_update_failed` |
| 托盘开关时 lan-sync 未注册 | 点了开关没生效 | warning | `quick_paste.tray_update_failed` |
| lan-sync 节点运行时错误 | 节点线程异常（启动失败 / 运行时错误，收不到/发不出） | error | `lan.peer_node_error` |
| lan-sync 收件箱持久化失败 | 收件箱写盘失败（丢数据风险） | error | `lan.storage_error` |

明确**不升级**的：窗口操作噪音（show/set_focus/cursor_position 失败）、读剪贴板格式失败（read_text/html/rtf/image）、emit 失败、快速粘贴 3 秒兜底隐藏——调试噪音或高频事件，通知会轰炸用户。

### 5.3 level 与前端渲染语义

| level | 前端视觉 | 自动消失 | 手动关闭 | 语义 |
| --- | --- | --- | --- | --- |
| `success` | 绿色 | 3 秒 | 不需要 | 完成类反馈（已写回 / 已保存） |
| `info` | 中性蓝 `#0a84ff` | 4 秒 | 不需要 | 中性提示（首版无后端消费方，枚举预留） |
| `warning` | 橙色 | 6 秒 | 需要 | 需用户决策的提示 |
| `error` | 红色 | 8 秒 | 需要 | 错误，需足够阅读时间 |

hover 暂停计时（用户正要看时不消失）。

### 5.4 前端 code 解析规则（映射表方案）

- 前端发起的通知：`code` 直接传 i18n 键（如 `clipboard.copied`、`quickPaste.saved`、`lanSync.writtenBack`）。
- 后端发起的通知：`code` 传规范错误码（snake_case，`lan.*` / `quick_paste.*`），前端 `notify` 模块内置**后端码 → i18n 键映射表**：

| 后端错误码 | i18n 键 |
| --- | --- |
| `clipboard.*` | 不变（两者一致） |
| `quick_paste.*`（除下条） | `quickPaste.*`（域名 camelCase） |
| `quick_paste.tray_update_failed` | `quickPaste.trayUpdateFailed`（键名也需映射） |
| `lan.*` | `lanSync.*` |

- 解析顺序：`t(code)` 直接命中 → 查映射表 → 兜底通用键 `notify.unknown`（带 `{code}` 参数，便于排查）。
- **顺带修复现存 bug**：`lan.*` / `quick_paste.*` 错误码此前在设置页/收件箱 `t(code)` 查不到 → 静默显示空串；映射表后正确翻译。
- **统一码不做**（后端错误码不动）：映射表仅 4 行 + 测试覆盖，改码要动契约文档/dt/前端，成本远高于收益。

### 5.5 前端 toast 堆栈行为

- 位置：主窗口右上角，竖排堆叠。
- 容量：同时最多 **4 条**；第 5 条到达时最早的一条**立即消失**（不排队）。
- 顺序：新条目标记出现在堆栈**顶部**，旧的向下推移（让位位移弹簧）。
- 去重（展示层）：同 `level + code` 且 **3 秒内**重复到达 → **不新增条目，重置该条计时器**（防止后端循环发或用户连续触发刷屏；与后端不节流不冲突）。
- 互斥：error 到达**不清掉**已有 success（语义不同，可共存）。
- 迁移后主窗三页（ClipboardHistory / Settings / Inbox）删除内联 `notice/error` 信号与 `<span class="message">` 渲染，操作反馈全部改为调用 `notify()`；**初次加载失败保留内联错误态**（toast 消失后空态会误导用户，见 5.6）。
- 无障碍：容器 `role="status"` + `aria-live="polite"`；关闭按钮 `aria-label="notify.dismiss"`；通知不抢焦点；`prefers-reduced-motion` → 150–200ms 纯 opacity 交叉淡化；`prefers-reduced-transparency` → 实色卡。
- 动效：进入 = 材质化（translateY 下落 + scale + opacity + blur 同步，临界阻尼无弹跳）；退出 = 同路径对称（上滑 + 淡出）；只动 transform/opacity。

### 5.6 前端迁移范围

| 页面 | 迁移 | 保留内联 |
| --- | --- | --- |
| ClipboardHistory | 回写成功（`clipboard.copied`）、删除/收藏/清空失败 | 初次加载失败 |
| Settings | 快捷键保存成功（`quickPaste.saved`）、终端名保存成功（`lanSync.saved`）、开关切换失败、保存失败 | 初次加载失败（getMaxEntries / getHotkey / getLanSyncStatus） |
| Inbox | 回写成功（`lanSync.writtenBack`）、删除/清空失败 | 初次加载失败 |
| QuickPastePopup（小屏） | **不迁移**（内联错误保持现状） | — |

### 5.7 测试组件（开发专用，T1）

- 设置页底部「通知测试」分组，`import.meta.env.DEV` 门控（`pnpm tauri dev` 可见，`pnpm build` 产物不渲染）。
- 控件：level 四选一（segmented）+ code 输入框（可输入任意键测试兜底路径）+ 可选 params 输入 + 「发送通知」按钮；完整走 `notify()` API → 后端 → 事件 → Provider 全链路，顺带验证 5.4 映射表与 `notify.unknown` 兜底。

## 6. 破坏性影响

- **无破坏性**：新增命令 `notify`、事件 `app://notify`、模块 `core/notify.rs`，不动既有命令/事件/存储。
- 前端内联通知迁移为内部改动（三个页面删信号与内联渲染）；小屏 popup 不受影响。
- 修复现存 bug：`lan.*` / `quick_paste.*` 错误码在设置页/收件箱静默消失（5.4）。
- 新文案：`notify.unknown`（带 `{code}`）、`notify.dismiss`，双语同步；其余复用现有键。
- 版本 0.2.8 三处同步（Cargo.toml / tauri.conf.json / package.json）。

## 7. 未决问题

- 无（「统一错误码」已判定不做，不挂 TODO）。
- 后续可选（不进首版）：通知中心（持久化列表）、level 图标、后端节流、toast 深度定制。
