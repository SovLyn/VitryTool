# 剪贴板历史（clipboard-history）

- 状态：已完成（0.1.0 签发）
- 接口契约：[docs/api/clipboard-history.md](../api/clipboard-history.md)
- 后端 mod：src-tauri/src/features/clipboard_history/
- 前端目录：src/features/clipboard-history/

## 目标

持续捕捉用户剪贴板变化，保存为可浏览、可回写的历史记录（文本 / HTML / RTF / 图片 / 文件引用，原始格式保真），受可设置的数量上限约束，并维持「记录 ↔ 图片文件」的一致性。

## 使用场景

- 复制文本/富文本/图片/文件后，应用内随时回看最近复制内容，点击条目即可写回剪贴板再次粘贴。
- 需要找回一段之前复制的文字、一张截图或一个文件引用。
- 临时不想让某些内容进入历史时，可单条删除或清空全部（暂停监听由后续全局快捷键提供，见 TODO）。

## 架构位置

```
src-tauri/src/features/clipboard_history/
├── mod.rs       # 模块声明与导出
├── commands.rs  # #[tauri::command] 薄壳（8 个命令）
├── service.rs   # 纯逻辑：去重置顶 / 即时淘汰 / 孤儿计算（无 IO，可独立测试）
├── store.rs     # 持久化抽象（HistoryStore）+ tauri-plugin-store 实现
└── tests.rs     # 开发者测试（dt）

src/features/clipboard-history/ClipboardHistory.tsx   # 主界面
src/api/clipboard-history.ts                          # invoke 封装（前端唯一入口）
```

关键依赖：`tauri-plugin-clipboard-x`（监听 + 读写 + 图片落盘）、`tauri-plugin-store`（历史与设置持久化）、`tauri-plugin-clipboard-x-api`（前端事件绑定）。

## 数据流

```
系统剪贴板变化
  → 插件 Rust 监听线程（startListening）
  → emit "plugin:clipboard-x://clipboard_changed"（仅到 WebView）
  → 前端 onClipboardChange
  → invoke captureClipboard
  → 后端：读各格式（逐格式容错）→ readImage 落盘 hash.png →
      内容指纹去重置顶 → 超限即时淘汰（删最旧条目+图片）→ store 写回
  → 前端刷新列表

定时兜底（前台发起，5 分钟）：
  前端 setInterval → invoke cleanupOrphanImages
  → 后端：扫描图片目录 vs 存活条目引用 → 删除孤儿图片
```

## 安全与边界

- 纯本地运行，数据存于应用数据目录（AppData）下：`clipboard.json`（历史+设置）、`tauri-plugin-clipboard-x/images/`（图片，插件默认路径）。
- 剪贴板历史**明文存储**敏感内容（密码、验证码等）：提供单条删除与清空全部；暂停监听规划为全局快捷键（TODO）。
- 文件格式仅记录源路径、不复制本体；源文件被移动/删除后引用失效。
- 图片缺失时条目保留并标记缺失（不悄悄删除）。
- 空内容/清空剪贴板事件静默忽略，不产生记录。
- 回写按项目内既有经验不触发监听（可能随插件版本变化，见契约未决问题）。

## 测试要点

- 后端 dt（`cargo test`）：内容指纹匹配各分支、去重置顶（id 保持/时间刷新/不淘汰）、即时淘汰（最旧优先/图片路径收集）、截断（setMaxEntries 语义）、孤儿差集（含 Windows 分隔符 `/` 与 `\` 表示不一致的回归测试）、MemoryStore 流程组合。
- 前端 vitest：invoke 封装（命令名与参数）、错误码提取、App 渲染（空状态/语言切换，宿主能力 mock）。
- 需要人工实测（真实剪贴板）：回写是否触发监听、快速连续复制是否丢失中间内容、图片去重置顶的 UI 表现。
- 图片预览依赖 asset 协议（`security.assetProtocol` 已配置且 scope 覆盖图片目录），加载失败回退占位——实机验证时留意。
