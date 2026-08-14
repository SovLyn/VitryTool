# 快速粘贴（quick-paste）

- 状态：已完成（0.2.0 签发；0.2.3 平台能力检测与设置页警告）
- 接口契约：[docs/api/quick-paste.md](../api/quick-paste.md)
- 后端 mod：src-tauri/src/features/quick_paste/
- 前端目录：src/features/quick-paste/
- 关联能力：系统托盘（src-tauri/src/core/tray.rs）、窗口状态记忆（tauri-plugin-window-state）

## 目标

用「按住快捷键 → 滚轮选择 → 松开即粘贴」的方式，把剪贴板历史变成真正的快速粘贴工具：无需打开主窗口，在任何应用中按住全局快捷键即可唤出置顶小屏，选中最近复制的内容并写回剪贴板。配套托盘常驻（关闭主窗口不退出）与窗口状态记忆（重启后恢复布局）。

## 使用场景

- 在文档 / 聊天窗 / 编辑器里连续粘贴多段之前复制过的文本：按住快捷键 → 滚轮选 → 松开 → Ctrl+V，循环往复。
- 找回一段被后来复制内容顶掉的旧文本 / 图片 / 文件引用。
- 关闭主窗口后应用仍在托盘常驻，随时唤出；重启后主窗口位置 / 大小复原。

## 架构位置

```
src-tauri/src/features/quick_paste/
├── mod.rs       # 模块声明与导出（含 init：挂载状态 + 注册已保存快捷键）
├── commands.rs  # #[tauri::command] 薄壳（4 个命令）+ 快捷键事件处理 + popup 窗口管理
├── service.rs   # 纯逻辑：快捷键解析 / 规范化 / 校验（无 IO，可独立测试）
├── store.rs     # 快捷键设置持久化（HotkeyStore，store 文件 quick-paste.json）
└── tests.rs     # 开发者测试（dt，会话状态机）

src-tauri/src/core/tray.rs                  # 托盘：菜单 / 左键唤出 / 退出前保存窗口状态
src/features/quick-paste/HotkeyRecorder.tsx # 快捷键录制组件（设置页使用）
src/features/quick-paste/QuickPastePopup.tsx# 置顶小屏页面组件（独立窗口）
src/features/quick-paste/popup.tsx          # popup 窗口入口（popup.html，Vite 多入口）
src/features/quick-paste/popup.css          # 小屏样式（透明窗口 + 玻璃卡片）
src/api/quick-paste.ts                      # invoke 封装（前端唯一入口）
src/features/settings/Settings.tsx          # 设置页新增「快捷操作」分组
```

关键依赖：`tauri-plugin-global-shortcut`（全局快捷键 Pressed / Released 事件）、`tauri-plugin-window-state`（窗口状态记忆）、`tauri-plugin-store`（快捷键设置持久化）、Tauri `tray-icon` feature（托盘）。小屏复用 clipboard-history 的 `getClipboardHistory` / `writeClipboardEntry` 命令，不新增剪贴板读写逻辑。

## 交互流

```
设置页录制快捷键（HotkeyRecorder）
  → setHotkey：后端校验（至少一个非 Shift 修饰键）→ 注销旧键 → 注册新键 → 持久化
  → 启动时 init 从 quick-paste.json 读取并自动注册

按住快捷键
  → Pressed（Rust）：会话 id +1 → 小屏定位到鼠标右下方（屏幕边界内 clamp）
  → show + focus → emit "quick-paste://show"
  → 小屏前端拉取剪贴板历史，选中第一项（最新）

滚轮 / ↑↓ → 切换选中（边界 clamp 不循环；选中项滚动到可见区域）

松开快捷键
  → Released（Rust）→ emit "quick-paste://release"（携带会话 id）
  → 小屏前端回写选中项 → quickPasteClose(会话 id) → 隐藏
  → 兜底：3 秒未关闭则强制隐藏；过期会话（id 不匹配）忽略

Esc（小屏内）→ 直接关闭，不回写
```

竞态处理：首次按下时 popup WebView 可能尚未加载完成——小屏挂载后调用 `quickPasteReady`，后端若存在挂起的按下事件则补发 show（契约 5.3）。

数据同步（0.2.1）：剪贴板捕捉为应用级监听（`listener.ts`），小屏 show 时先补一次 `captureClipboard`；小屏激活期间收到 `clipboard-history://updated` 实时刷新列表并保持当前选中条目，未激活时不刷新。

## 托盘与窗口

- 主窗口与 popup 窗口的 `CloseRequested` 一律改为隐藏（`prevent_close` + `hide`），进程常驻。
- 托盘：左键单击 / 双击唤出主窗口；菜单「显示主窗口」「退出」；「退出」前显式 `save_window_state`（`app.exit` 不触发 close 事件，需手动落盘）。
- 窗口状态记忆：主窗口位置 / 大小 / 最大化默认全部记录（插件在 Moved / Resized 时防抖保存），重启自动恢复；popup 小屏在 `with_denylist` 中排除，每次跟随鼠标定位。
- popup 窗口：预创建于 `tauri.conf.json`（label `quick-paste`），透明、无边框、置顶、跳过任务栏、初始隐藏、不可缩放；每次显示定位到鼠标附近，隐藏而非销毁（下次秒开）。

## 平台限制（0.2.3）

- **Linux Wayland 会话不支持全局快捷键**：`tauri-plugin-global-shortcut` 底层 `global-hotkey` 仅实现 X11 后端（`XGrabKey`）。Wayland 会话中窗口为原生 Wayland，键盘事件不经过 X server——快捷键注册「成功」（XWayland 存在时）但按下永不触发（Linux 实测确认）。
- 应用启动时（设置页）调用 `getHotkeyCapability` 检测：`supported=false` 时设置页不提供快捷键录制，改为显示警告（提示切换 X11 会话或设置 `GDK_BACKEND=x11`）。检测逻辑为后端纯函数 `service::global_shortcut_supported`，见契约 5.8。
- 兜底说明：即使 `GDK_BACKEND=x11`（GTK 走 XWayland），仍依赖合成器对 XWayland 抓键的支持，不作为保证；用户应优先切换到 X11 会话使用本功能。

## 安全与边界

- 快捷键**必须包含至少一个非 Shift 修饰键**（Ctrl / Alt / Win），禁止裸字母 / 仅 Shift 组合，避免拦截常规输入。
- 快捷键设置持久化于 `AppData/quick-paste.json`（仅存快捷键字符串，不含剪贴板内容）。
- 回写复用 `writeClipboardEntry`（按原始格式写回）；按既有经验不触发监听，若触发则去重置顶自然置顶，无害。
- 托盘菜单文案为后端硬编码中文（Rust 侧无 i18n 基建，见契约未决问题）。
- 小屏为置顶窗口，展示内容与剪贴板历史一致（可能含敏感信息），使用完毕后自动隐藏。

## 测试要点

- 后端 dt（`cargo test`）：快捷键规范化（大小写 / 别名 / 修饰键去重 / 顺序固定）、校验拒绝分支（无修饰键 / 仅 Shift / 无主键 / 未知 token / 两个主键）、会话状态机（自增 / 防过期误关 / 重复按下幂等）、平台能力检测（Wayland 默认不支持 / `GDK_BACKEND=x11` 例外 / X11 会话支持 / 会话变量缺失分支）。
- 前端 vitest：invoke 封装（含 `getHotkeyCapability`）、HotkeyRecorder（录制组合 / 纯修饰键忽略 / 非法组合提示 / Esc 取消 / 展示格式化）、QuickPastePopup（show 补捕捉与高亮 / wheel 切换与 clamp / release 回写选中并 close / 过期会话忽略 / 空历史 / Esc 取消 / updated 实时刷新保持选中 / 未激活不刷新）、设置页能力分支（`supported=false` 显示警告且无录制入口 / 支持时正常 / 检测失败 fail-open）、i18n 跨窗口 storage 同步。
- 需要人工实测（真实环境）：
  - 全局快捷键在**其他应用窗口上方**是否生效（焦点不在 VitryTool）；
  - **设置页切语言 → 复制新内容 → 唤出小窗：新内容应出现**（0.2.1 修复点）；
  - **小窗激活期间在别处复制新内容 → 小窗实时出现并保持选中**（0.2.1 修复点）；
  - **英文状态下小窗文案应显示英文**（0.2.1 修复点）；
  - 回写后是否触发监听、是否会把自己写回的内容再次捕捉；
  - 透明小屏在 Windows 上的显示效果（无边框圆角 / 置顶层级）；
  - 托盘「退出」后下次启动主窗口位置 / 大小是否复原。
