# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 与语义化版本约定（见 `docs/versioning.md`）。

## [0.2.8] - 2026-08-16

### 新增

- **全局通知系统（notify）**，契约见 `docs/api/notify.md`、功能文档 `docs/features/notify.md`：
  - **双向统一通道**：前端经新命令 `notify(level, code, params?)` 提交通知 → 后端校验（level ∈ success/error/warning/info、code 非空，非法返回 `notify.invalid`）→ 广播 `app://notify` 事件到所有窗口；后端内部站点也可直接调用 `core::notify::notify_app` 发通知。负载为结构化 `level + code + params`，**不含界面文案**（符合「后端不输出界面文案」铁律），前端渲染时按当前 locale 翻译（切语言即时重译）。
  - **后端 5 个站点接入**（此前只记日志、用户无感知）：快捷键注册失败（`quick_paste.register_failed`，error）、托盘广播/接收开关失败（`quick_paste.tray_update_failed`，error）、托盘开关时 lan-sync 未注册（同码，warning）、lan-sync 节点线程异常退出（`lan.peer_node_error`，error）、收件箱持久化失败（`lan.storage_error`，error）；全部只 emit 不阻塞；正常退出前置位关闭标记避免误报节点错误。
  - **前端 NotificationProvider**（`src/components/NotificationProvider.tsx`，仅主窗口挂载）：右上角玻璃 toast 堆栈（`--surface-raised` + backdrop-filter + level 强调条/色点），分 level 自动消失（success 3s / info 4s / warning 6s / error 8s）、hover 暂停计时、warning/error 带手动关闭、最多 4 条、新到顶部（FLIP 让位）、同 level+code 3 秒内去重置顶计时；进入材质化动效（translateY+scale+opacity+blur，临界阻尼）、退出对称上滑淡出；`role="status"` + `aria-live="polite"`、关闭按钮 aria-label、`prefers-reduced-motion`/`prefers-reduced-transparency` 降级。
  - **前端三页迁移**（ClipboardHistory / Settings / Inbox）：操作反馈（回写/保存/开关/删除/清空/收藏的成功与失败）全部改为 `notify()`，删除页面内联 notice 信号与渲染；仅**初次加载失败**保留内联错误态（toast 消失会留下误导空态）。小屏 popup 不迁移（瞬态窗口，简略为要）。
  - **错误码解析映射表**：`src/api/notify.ts` 内置后端码 → i18n 键映射（`lan.*`→`lanSync.*`、`quick_paste.*`→`quickPaste.*`），**修复现存 bug**：lan-sync 错误码（`lan.*`）此前在设置页/收件箱 `t()` 查不到 i18n 键 → 错误静默消失，现在正确翻译。
  - **通知测试组件**（设置页底部「通知测试」分组，`import.meta.env.DEV` 门控，发布构建不渲染）：自定义 level / code / params 走完整 `notify()` 链路，可验证映射表与 `notify.unknown` 兜底。
  - 新文案：`notify.unknown`（带 `{code}` 参数）、`notify.dismiss`、通知测试相关键，zh-CN / en-US 双语同步。
  - 后端 dt +9（notify level 解析 / 参数校验 / payload 序列化形状），前端 vitest 新增 26 用例（api 映射表 9 + Provider 行为 16 + 页面迁移断言 1）；clippy 干净。

### 修复

- **通知频闪（实测发现）**：计时器此前每 200ms 为每个活跃 toast 创建新对象递减剩余时长，`<For>` 按对象引用 keyed → 整个列表 DOM 每 tick 重建一次，CSS 进入动画随之反复重放（每次从 opacity 0 起播），表现为「频繁出现-消失」。重构为 **deadline（到期时间戳）模式**：tick 只做到期检查，未到期时完全不调用 `setToasts`，DOM 永不重建；hover 暂停改为累计暂停偏移（`pausedMs`），暂停/恢复不触碰 toast 状态、恢复不触发渲染。新增回归测试（tick 期间元素引用稳定）。
- **确认对话框替代 `window.confirm`（实测发现）**：收件箱 / 剪贴板历史的「清空」确认此前用 `window.confirm`，WebView 原生对话框显示 "localhost:1420 显示" 宿主标题。新增纯前端 `ConfirmDialog` 组件（`src/components/ConfirmDialog.tsx`）：模态遮罩 + 玻璃卡（沿用全局视觉）、破坏性操作红色确认按钮（`.btn-danger`，macOS alert 惯例）、`role="alertdialog"` + 焦点管理（打开聚焦取消、Esc / 遮罩点击取消、关闭还原焦点）、进入材质化动效 + reduced-motion 降级；新增 `common.cancel` 双语键。两处「清空」接入。

### 变更

- 版本 0.2.7 → 0.2.8（三处同步）。
- 移除三页内联成功/错误消息渲染（`.message notice/error`），统一走全局通知；`error` 信号语义收窄为「初次加载失败」。

## [0.2.7] - 2026-08-16

### 新增

- **托盘 lan-sync 快速开关**（契约 `docs/api/quick-paste.md` 5.5、`docs/api/lan-sync.md` 5.7）：托盘菜单新增「剪贴板广播」「剪贴板接收」两个可勾选项（CheckMenuItem），勾选态反映当前开关，点击即切换并持久化——经 `core::hooks` 新增的开关钩子（`register_lan_sync_switches`）读写，与设置页 `setLanSyncBroadcast` / `setLanSyncReceive` 同一共享态与持久化路径；文案随 `setTrayLabels` 由前端 i18n 下发（新增 `tray.broadcast` / `tray.receive` 双语键）。后端 dt +2（开关钩子未注册返回 None / 注册后委托函数），前端 vitest 断言更新。
- **设置实时同步（0.2.7）**：托盘或设置页切换广播/接收后，后端 emit `lan-sync://settings-updated`，设置页监听该事件重新拉取 `getLanSyncStatus` 刷新开关状态——托盘切换后无需重进设置页即可看到最新状态。
- **品牌图标落地**：改用用户提供的 `src-tauri/icons/galaxy.svg`（唯一设计源），删除前端脚手架默认图标（`public/tauri.svg`、`public/vite.svg`、`src/assets/logo.svg` 默认内容），`index.html` favicon 指向 galaxy；dev/打包窗口图标经 tauri-build 读取 `icons/icon.ico` 自动生效。

### 变更

- 版本 0.2.6 → 0.2.7（三处同步）。
- `setTrayLabels` 命令参数由 2 个扩展为 4 个（showMain / quit / broadcast / receive），契约 quick-paste 5.5 与命令表同步。

## [0.2.6] - 2026-08-16

### 新增

- **托盘菜单文案接入 i18n**（契约 `docs/api/quick-paste.md` 5.5）：菜单文案由前端 i18n 提供，主窗口加载后及语言切换时经新命令 `setTrayLabels` 下发，后端不持有界面文案（符合「后端不输出界面文案」铁律）；错误码 `quick_paste.tray_update_failed`（双语文案）。后端 `set_tray_labels` 命令 dt 3 组（文案校验：合法 / 空与纯空白拒绝 / trim 后判空），前端新增 api 封装用例与 App 挂载下发断言。

### 变更

- 版本 0.2.5 → 0.2.6（三处同步）。
- `tauri.conf.json` 构建命令由 `deno task dev/build` 改为 `pnpm dev` / `pnpm build`（仓库无 `deno.json`，此前 `pnpm tauri dev` 会失败；README 亦为 pnpm 方式）。
- 回填仓库 URL：README issue 链接与 Cargo.toml `repository` 指向 `https://github.com/SovLyn/VitryTool`。
- 新增 CI：`.github/workflows/ci.yml`（fmt + clippy + cargo test + vitest + tsc + pnpm build）。
- 新增 CD：`.github/workflows/release.yml`（打 `v*` tag 触发 → tauri-action 三平台构建安装包 → GitHub Release 草稿，人工确认后发布）。
- **品牌图标**：替换 Tauri 默认图标——SVG 源（`src-tauri/icons/galaxy.svg`）+ `scripts/render-icon.mjs`（resvg 渲染 1024 PNG，含居中旋转修复）+ `tauri icon` 生成全套（ico/icns/png）；设计：蓝色轨道环 + 中心球（SVG Repo 资源，象征局域网互联）；品牌规范见 `docs/design/brand.md`。新增 devDependency `@resvg/resvg-js`。

## [0.2.5] - 2026-08-14

### 新增

- **局域网剪贴板同步（lan-sync）**，契约见 `docs/api/lan-sync.md`：
  - **节点层（`core/peer_node`，跨功能复用）**：libp2p 0.56（mDNS 发现 + TCP/QUIC 连接 + gossipsub 广播），独立 tokio 线程随应用生命周期运行；ed25519 身份持久化（`AppData/peer-key.json`，peerId 为终端稳定身份，不依赖 IP）；固定主题 `vitrytool-lan-clipboard`，信封 `v=0.2.5` 向后兼容（只增字段）。
  - **单实例**（tauri-plugin-single-instance）：一台机器一个终端；第二实例启动唤出主窗口。
  - **复制即广播**：剪贴板历史产生新条目（`is_new`）时经 `core::hooks` 通知广播（防环 / 开关 / 1MiB 体积上限在 lan-sync 侧判断；超限静默跳过）。
  - **收件箱**：按来源节点分桶（每桶最新 8 条，全局最多 8 个节点桶，新节点淘汰「桶内最新条目最旧」的整桶）；指纹去重置顶；本机广播不入箱；持久化 `AppData/lan-inbox.json`；事件 `lan-sync://inbox-updated` 驱动前端刷新。
  - **防环**：近期接收指纹 LRU（100 条），回写/系统回环不重广播。
  - **命令面**：`getLanSyncStatus` / `setLanSyncBroadcast` / `setLanSyncReceive` / `setLanSyncTerminalName` / `getLanInbox` / `writeLanInboxEntry` / `deleteLanInboxEntry` / `clearLanInbox`；开关默认全开（`AppData/lan-sync.json`）。
  - **前端**：主窗口新增「收件箱」页（节点分组列表、单击回写、hover 删除、新条目高亮脉冲、粘性磨砂分组头、空态引导）；侧栏未读徽标；设置页新增「局域网同步」区（广播/接收开关、终端名、在线终端数、本机 ID）。
  - 内容范围：文本 / HTML / RTF / 文件路径广播；**图片首版仅广播元数据**（名称/尺寸，字节传输 TODO）。
  - 后端 dt 新增 20 组（收件箱分桶/去重/全局淘汰/信封映射/指纹/身份持久化等），前端新增 vitest 12 用例；clippy 干净。

### 变更

- 版本 0.2.4 → 0.2.5（三处同步）。
- 依赖新增：`libp2p`、`tokio`、`futures`、`tauri-plugin-single-instance`。
- `capture_clipboard` 在产生新条目时调用 `core::hooks::notify_new_entry`（未注册为空操作，不影响既有行为）。
- README 已知限制新增：Windows 虚拟网卡（尤其 WSL 虚拟交换机）可能使 mDNS 发现失败（实测关闭 WSL 虚拟交换机后恢复；临时规避改 metric/删路由）；广播单条上限 1MiB。
- `dev/CONTEXT.md` 新增 lan-sync 领域术语；前期调研与决策记录见 `dev/interface-drafts/`。



### 新增

- **剪贴板收藏（Favorite）**（契约 `docs/api/clipboard-history.md` 5.8）：
  - 条目新增 `favoritedAt` 字段（收藏时刻，存在即收藏；旧数据 serde `default` 零迁移）。
  - 新命令 `setEntryFavorite(id, favorited)`（幂等；重复收藏刷新收藏时间，收藏区重新置顶）。
  - 收藏条目**豁免数量上限**：`maxEntries` 只约束非收藏条目，`evict_over_limit` 仅淘汰最旧的非收藏条目；取消收藏不触发淘汰（容忍短暂超限，下次捕捉/调上限归位）。
  - `getClipboardHistory` 后端排序返回：收藏区在前（区内按收藏时间倒序），其后按捕捉时间倒序（`service::sort_for_display` 稳定排序，两窗共用）。
  - 收藏条目计入孤儿图片判定引用（条目存活期间图片不被清理）；收藏不豁免主动清空与显式删除。
  - **主窗口**：收藏区分组标题 + 卡片左侧 accent 强调条 + 星标按钮（`aria-pressed`）；收藏/取消收藏后复用 `clipboard-history://updated` 事件跨窗同步。
  - **快速粘贴小屏**：`F` 键或选中条目星标按钮切换收藏（选中行星标反色为 accent-text）；提示行更新。
  - 后端 dt 新增 9 组（展示排序 / 收藏豁免淘汰 / set_favorite 幂等与刷新 / 旧数据兼容 / 收藏流程组合），前端新增 vitest 用例（API 封装 + 小屏 F 键/星标按钮）。

### 变更

- 版本 0.2.3 → 0.2.4（三处同步）。
- `dev/CONTEXT.md` 新增「收藏（Favorite）」术语与规则。

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
