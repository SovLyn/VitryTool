# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 与语义化版本约定（见 `docs/versioning.md`）。

## [0.2.3] - 2026-08-14

### 新增

- **全局快捷键平台能力检测**（契约 `docs/api/quick-paste.md` 5.8）：新增 `getHotkeyCapability` 命令，检测当前环境是否支持全局快捷键。Linux 下 `global-hotkey` 仅实现 X11 后端（`XGrabKey`）——Wayland 会话中窗口为原生 Wayland，键盘事件不经过 X server，快捷键注册「成功」但按下永不触发（实测确认）。
- **设置页平台警告**：能力检测 `supported=false`（如 Linux Wayland 会话）时，快捷键设置区不再提供录制入口，改为显示警告（提示切换 X11 会话或设置 `GDK_BACKEND=x11`），避免用户配置一个永远不生效的快捷键；文案中英文双语（`quickPaste.unsupportedTitle` / `unsupportedDesc`）。
- 后端新增可测纯函数 `service::global_shortcut_supported`（注入环境变量，dt 覆盖 Wayland / X11 / GDK_BACKEND 强制 X11 等分支）。

### 变更

- 后端日志补全：`popup.show()` / `set_focus()` / `cursor_position()` / `outer_size()` / `monitor_from_point()` / `set_position()` / 事件 `emit` 失败不再被吞掉，均记录错误日志（此前 `let _` 静默丢弃，掩盖跨平台窗口问题）。
- 版本 0.2.2 → 0.2.3（三处同步）。

## [0.2.2] - 2026-08-13

### 修复

- **小窗条目类型标记贴右**：小窗列表中「文本 / 图片 / 文件」等类型标记固定在最右端——图片条目此前为裸 `<img>`（无 `flex: 1` 撑开），类型标记会紧跟图片；现图片统一包裹在 preview 容器中，并给类型标记加 `margin-left: auto` 兜底。

## [0.2.1] - 2026-08-13

### 修复

- **快速粘贴小窗数据不同步**：
  - 剪贴板监听提升为应用级（`listener.ts`，App 挂载时启动），不再绑定在历史页组件生命周期——切到设置页或主窗口隐藏期间复制的内容也会进入历史；
  - 小窗 show 时先补一次 `captureClipboard`（兜底主窗口未捕捉到的最新复制）；
  - 小窗激活期间监听 `clipboard-history://updated` 实时刷新（保持当前选中条目）；
  - 后端 `captureClipboard` 加互斥锁，主窗口与小窗并发捕捉同一内容不再重复插入。
- **小窗语言不随主窗口切换（i18n）**：i18n 增加跨窗口 `storage` 事件同步，小窗（独立 I18nProvider 实例）跟随主窗口语言切换；主题同理（`theme.tsx` 模块级 storage 监听）。

### 变更

- `ClipboardHistory` 组件改为事件驱动刷新（监听 `clipboard-history://updated`），不再自持监听与定时器。
- 契约文档同步：`docs/api/clipboard-history.md`（数据流与应用级监听）、`docs/api/quick-paste.md`（5.3 小屏数据实时同步）。

## [0.2.0] - 2026-08-13

### 新增

- **快速粘贴（quick-paste）**，契约见 `docs/api/quick-paste.md`：
  - 快捷键录制组件（HotkeyRecorder）：设置页录制全局快捷键（标准格式持久化，启动自动注册；要求至少一个非 Shift 修饰键，防止拦截常规输入）。
  - 按住快捷键唤出**置顶小屏**（跟随鼠标、透明无边框、跳过任务栏、初始隐藏），展示剪贴板历史列表。
  - 滚轮 / ↑↓ 切换选中项（边界 clamp 不循环）；**松开快捷键**将选中项按原始格式回写剪贴板并关闭小屏；小屏内 Esc 取消。
  - 首次按下时 WebView 未加载完的竞态握手（quickPasteReady 补发 show）；前端异常时后端 3 秒兜底隐藏；会话 id 防过期回调误关新会话。
- **系统托盘**：关闭主窗口改为隐藏（进程常驻），托盘左键单击唤出，菜单「显示主窗口」「退出」；退出前显式保存窗口状态。
- **窗口状态记忆**（tauri-plugin-window-state）：主窗口位置 / 大小 / 最大化状态重启后恢复；快速粘贴小屏不参与记忆（每次跟随鼠标）。

### 变更

- 依赖新增：`tauri-plugin-global-shortcut`、`tauri-plugin-window-state`（Rust）；`tauri` 启用 `tray-icon` feature。
- `tauri.conf.json` 新增 `quick-paste` 窗口（透明 / 无边框 / 置顶 / 跳过任务栏）；capabilities 新增 `quick-paste`；Vite 双入口（`index.html` + `popup.html`）。
- 托盘菜单文案后端硬编码中文（暂不接入 i18n，见契约未决问题）。

## [0.1.2] - 2026-08-13

### 新增

- **主题系统**：亮色 / 暗色 / 跟随系统，`src/theme.tsx`（localStorage 持久化 + `matchMedia` 实时跟随 + 首帧无闪烁）。
- **独立设置页**：语言、主题、剪贴板条数上限；语言/主题纯前端持久化。
- **左侧标签栏导航框架**：功能在上、设置固定在底部，为后续功能预留。
- **Apple 风格视觉系统**：亮暗两套语义色变量 + 玻璃材质（backdrop-filter）贯穿全局 + 即时按压反馈 + 排版层级。

### 变更

- 语言设置持久化到 localStorage（此前切语言不记忆）。
- 剪贴板条数上限改为**失焦即存**（移除保存按钮与成功提示；无效输入恢复原值）。
- 移除主界面「最多保留 N 条」提示。
- 设置由弹窗改为独立页面。

## [0.1.1] - 2026-08-13

### 新增

- **日志系统**：基于 `log` + `tauri-plugin-log`（`core/log.rs`）。开发构建输出 Trace 级到终端（系统 crate 压到 Info）；发布构建保存 Error 级到应用日志目录文件（5 MiB 轮转、保留最近一份）。开发构建同时将 Trace 落盘（便于排查）。
- 剪贴板历史各命令补齐日志（元数据为主，遵循隐私约束，不记录剪贴板明文）。

### 修复

- **孤儿清理误删图片**：`image_dir` 用单个含 `/` 的相对串 `join`，在 Windows 上保留正斜杠，与插件落盘路径（`\`）不一致，导致 `orphan_files` 字符串比较失败、全部图片被误判为孤儿删除。现改为分开 `join`，且路径比较改用 `Path::components`（`/` 与 `\` 视为同一分隔符），并加回归测试。
- **去重丢弃的已落盘图片成为孤儿**：`read_image` 提前落盘，但去重命中旧条目时新图可能不被采纳；现于 `capture_clipboard` 内清理本次落盘且未被任何条目引用的图片。

### 变更

- `docs/architecture.md` 新增「日志约定」章节。

## [0.1.0] - 2026-08-13

### 新增

- **首个功能：剪贴板历史（clipboard-history）**，契约见 `docs/api/clipboard-history.md`：
  - 全格式捕捉：文本 / HTML / RTF / 图片 / 文件引用，原始格式保真记录，时间戳必记。
  - 内容指纹去重置顶；数量上限可设置（默认 64，最大 1024），超限即时淘汰。
  - 图片由 tauri-plugin-clipboard-x 落盘于插件默认目录；前台定时（5 分钟）兜底清理孤儿图片。
  - 点击条目按原始格式回写剪贴板；支持单条删除与清空全部。
  - 启动即自动监听；图片缺失条目保留并标记。
- 架构决策记录：`docs/adr/0001-clipboard-capture-via-webview-events.md`（捕捉链路经前端事件驱动）。
- 领域术语表：`dev/CONTEXT.md`（内部文档，不对外发布）。

### 变更

- 移除脚手架 `greet` 命令与前端演示。
- 依赖新增：`tauri-plugin-clipboard-x`、`tauri-plugin-store`（Rust）、`tauri-plugin-clipboard-x-api`（前端）。
- 前端主界面由演示页切换为剪贴板历史页面。
- 修复图片预览无法加载：启用 `security.assetProtocol`（scope 限定图片目录），`<img>` 加载失败回退占位文案。

### 已知限制（后续迭代）

- 大列表（接近 1024 条富文本）时 store 全量序列化存在性能开销。
- 快速连续复制时，Windows 剪贴板监视延迟可能丢失中间内容。

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
