# 通知系统（notify）

- 状态：已完成（0.2.8）
- 接口契约：[docs/api/notify.md](../api/notify.md)
- 后端 mod：`src-tauri/src/core/notify.rs`（横切基础，非功能域）
- 前端目录：`src/components/NotificationProvider.tsx` + `src/api/notify.ts`

## 目标

让应用内的反馈（成功 / 错误 / 警告 / 信息）走**一条统一通道**：前端操作结果与后端内部错误都进入同一个 `app://notify` 事件，由主窗口右上角的 toast 堆栈集中渲染。替代各页面各自为政的内联消息，并为「后端错误推给用户」提供正式接口。

## 使用场景

- 用户在设置页保存快捷键 → 右上角绿色 toast「快捷键已保存」；注册失败（快捷键被占用）→ 红色 toast「快捷键注册失败…」。
- 用户从托盘快速开关广播/接收，切换失败 → 红色 toast（此前只有日志，用户无感知）。
- 收件箱回写成功 → 绿色 toast「已写回剪贴板」；初次加载失败 → 页面内联错误态（toast 消失会留下误导空态）。
- 开发期：设置页「通知测试」分组（DEV 构建可见）可发任意 level/code 的通知，验证全链路与兜底翻译。

## 架构位置

- 后端 `core/notify.rs`（横切基础，与 `core/tray.rs` 平级）：命令 `notify`（前端提交入口）+ `notify_app`（后端内部站点直发）+ `NotifyPayload` / `NotifyLevel` 类型。命令注册进 `lib.rs` 的 `invoke_handler`。
- 前端 `src/api/notify.ts`：`notify()` invoke 封装 + `APP_NOTIFY_EVENT` 常量 + **后端码 → i18n 键映射表**（`lan.*`→`lanSync.*`、`quick_paste.*`→`quickPaste.*`）。
- 前端 `src/components/NotificationProvider.tsx`：仅主窗口挂载（`App.tsx`），监听事件、维护 toast 堆栈（容量 4 / 分 level 时长 / 去重 / 让位），渲染时按当前 locale 翻译。
- 页面侧：ClipboardHistory / Settings / Inbox 删除内联 `notice/error` 信号，操作反馈改调 `notify()`；初次加载失败保留内联。

## 数据流

```
页面操作成功/失败 ──notify({level, code, params})──▶ invoke("notify")
后端内部站点（托盘开关失败等） ──notify_app(app, level, code)──┐
                                                               ▼
                                                      app.emit("app://notify")
                                                               ▼
                                     NotificationProvider（主窗）listen → 渲染 toast
                                                               ▼
                                     t(code) → 映射表 → notify.unknown 兜底（渲染时翻译）
```

## 安全与边界

- 后端不输出界面文案：负载只有 `level + code + params`，文案全部由前端 i18n 渲染（符合「后端不输出界面文案」铁律）。
- 通知失败不影响主流程：`notify` 命令 emit 失败仅记日志，前端 fire-and-forget。
- 不持久化、不分配 id、不节流——toast 即用即走，通知中心是以后的事。
- 小屏 popup 不渲染通知（瞬态窗口，简略为要）。

## 测试要点

- 后端 dt：`core/notify.rs`——level 解析（四值 / 非法）、code 校验（空/纯空白拒绝）、payload 序列化形状。
- 前端 vitest：`src/api/notify.test.ts`（映射表：`lan.*`→`lanSync.*`、`quick_paste.tray_update_failed`→`quickPaste.trayUpdateFailed`、未知码落 `notify.unknown`）；`NotificationProvider.test.tsx`（容量 4 挤掉最早、同码 3 秒去重置顶计时、分 level 时长自动消失、warning/error 有关闭按钮、success/info 无、aria-live 属性、**防频闪回归**：tick 推进期间 DOM 元素引用稳定，防止进入动画反复重放）。
- **防频闪设计（实测修复）**：toast 持到期时间戳（deadline），计时 tick 只做到期检查，未到期时完全不调用 `setToasts`——`<For>` 按对象引用 keyed，引用不变则 DOM 不重建、CSS 进入动画不重放（否则每 200ms 全列表重建 = 持续频闪）；hover 暂停用累计暂停偏移（pausedMs）实现，暂停/恢复不触碰 toast 状态。
- 既有页面测试：ClipboardHistory / Settings / Inbox 断言随迁移更新（内联消息 → notify 调用）。
- 人工实测：dev 构建设置页「通知测试」发各 level 通知，验证显示/时长/关闭/切语言重译；托盘开关失败通知；快捷键占用失败通知。
