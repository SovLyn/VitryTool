# 剪贴板历史（clipboard-history）

- 状态：已完成（0.1.0 签发；0.2.4 新增收藏）
- 接口契约：[docs/api/clipboard-history.md](../api/clipboard-history.md)
- 后端 mod：src-tauri/src/features/clipboard_history/
- 前端目录：src/features/clipboard-history/

## 目标

持续捕捉用户剪贴板变化，保存为可浏览、可回写的历史记录（文本 / HTML / RTF / 图片 / 文件引用，原始格式保真），受可设置的数量上限约束，并维持「记录 ↔ 图片文件」的一致性。

## 使用场景

- 复制文本/富文本/图片/文件后，应用内随时回看最近复制内容，点击条目即可写回剪贴板再次粘贴。
- 需要找回一段之前复制的文字、一张截图或一个文件引用。
- **收藏常用内容**（0.2.4）：星标收藏的条目置顶展示、带特殊外观、豁免数量上限淘汰，主窗口与小屏均可收藏/取消收藏（小屏按 `F` 或点星标）。
- 临时不想让某些内容进入历史时，可单条删除或清空全部（暂停监听由后续全局快捷键提供，见 TODO）。

## 架构位置

```
src-tauri/src/features/clipboard_history/
├── mod.rs       # 模块声明与导出
├── commands.rs  # #[tauri::command] 薄壳（9 个命令，capture / setEntryFavorite 带互斥锁）
├── service.rs   # 纯逻辑：去重置顶 / 即时淘汰 / 收藏 / 展示排序 / 孤儿计算（无 IO，可独立测试）
├── store.rs     # 持久化抽象（HistoryStore）+ tauri-plugin-store 实现
└── tests.rs     # 开发者测试（dt）

src/features/clipboard-history/listener.ts        # 应用级监听（App 挂载启动，捕捉 + 广播 updated 事件）
src/features/clipboard-history/ClipboardHistory.tsx # 主界面（列表/回写/删除/清空，事件驱动刷新）
src/features/settings/Settings.tsx                # 设置页（语言/主题/条数）
src/api/clipboard-history.ts                      # invoke 封装（前端唯一入口）
src/theme.tsx                                     # 主题系统（亮/暗/跟随系统）
```

前端导航：左侧标签栏（功能在上、设置固定在底部，见 `src/App.tsx`），玻璃材质为贯穿全局的视觉语言（`src/App.css` 的语义色变量 + backdrop-filter）。

关键依赖：`tauri-plugin-clipboard-x`（监听 + 读写 + 图片落盘）、`tauri-plugin-store`（历史与设置持久化）、`tauri-plugin-clipboard-x-api`（前端事件绑定）。

## 数据流

```
系统剪贴板变化
  → 插件 Rust 监听线程（startListening）
  → emit "plugin:clipboard-x://clipboard_changed"（仅到 WebView）
  → 应用级监听（listener.ts，App 挂载时启动，不随页面切换停止）
  → invoke captureClipboard（后端持互斥锁，读各格式 → 落盘 hash.png →
      内容指纹去重置顶 → 超限即时淘汰 → store 写回）
  → 前端广播 "clipboard-history://updated"
  → 历史页 / 快速粘贴小屏（激活时）据此刷新列表

定时兜底（前台发起，5 分钟，应用级）：
  setInterval → invoke cleanupOrphanImages
  → 后端：扫描图片目录 vs 存活条目引用 → 删除孤儿图片
```

监听不再绑定在 `ClipboardHistory` 组件生命周期（0.2.1 起）：用户在设置页或
主窗口隐藏（托盘常驻）期间复制的内容也会被捕捉。主窗口 WebView 隐藏后仍存活，
监听与定时器继续工作（ADR 0001 推论成立）。

## 安全与边界

- 纯本地运行，数据存于应用数据目录（AppData）下：`clipboard.json`（历史+设置）、`tauri-plugin-clipboard-x/images/`（图片，插件默认路径）。
- 剪贴板历史**明文存储**敏感内容（密码、验证码等）：提供单条删除与清空全部；暂停监听规划为全局快捷键（TODO）。
- 文件格式仅记录源路径、不复制本体；源文件被移动/删除后引用失效。
- 图片缺失时条目保留并标记缺失（不悄悄删除）。
- 空内容/清空剪贴板事件静默忽略，不产生记录。
- 回写按项目内既有经验不触发监听（可能随插件版本变化，见契约未决问题）。

## 测试要点

- 后端 dt（`cargo test`）：内容指纹匹配各分支、去重置顶（id 保持/时间刷新/不淘汰）、即时淘汰（最旧优先/图片路径收集/**收藏豁免**）、截断（setMaxEntries 语义）、孤儿差集（含 Windows 分隔符 `/` 与 `\` 表示不一致的回归测试）、MemoryStore 流程组合；0.2.4 新增收藏 dt（展示排序、收藏豁免淘汰、set_favorite 幂等与刷新、旧数据 serde 零迁移）。
- 前端 vitest：invoke 封装（命令名与参数，含 `set_entry_favorite`）、错误码提取、小屏 `F` 键/星标按钮收藏、App 渲染（空状态/语言切换，宿主能力 mock）。
- 需要人工实测（真实剪贴板）：回写是否触发监听、快速连续复制是否丢失中间内容、图片去重置顶的 UI 表现、小屏 `F` 键与星标按钮的实操手感。
- 图片预览依赖 asset 协议（`security.assetProtocol` 已配置且 scope 覆盖图片目录），加载失败回退占位——实机验证时留意。
