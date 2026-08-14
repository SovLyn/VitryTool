# 功能名：快速粘贴（quick-paste）

- 状态：`已实现`
- 关联功能文档：[docs/features/quick-paste.md](../features/quick-paste.md)
- 版本影响：`minor`（0.1.2 → 0.2.0，已签发）；`patch`（0.2.2 → 0.2.3，已签发：平台能力检测 + 设置页警告）；`patch`（0.2.3 → 0.2.4，已签发：小屏收藏交互，见 5.7）

## 1. 概述

按住全局快捷键唤出一个**置顶小屏**（Quick Paste Popup），展示剪贴板历史（复用 clipboard-history 的条目与回写能力）；滚轮 / 方向键切换选中项，**松开快捷键**时将选中项按原始格式回写剪贴板并关闭小屏；小屏内可收藏 / 取消收藏选中条目（`F` 键或星标按钮，见 5.7）。配套三件事：

1. **快捷键录制组件**（HotkeyRecorder）：在设置页录制并保存全局快捷键；
2. **系统托盘**：关闭主窗口改为隐藏（进程常驻），托盘唤出 / 退出；
3. **窗口状态记忆**（tauri-plugin-window-state）：重启后恢复主窗口位置 / 大小。

领域术语沿用 `dev/CONTEXT.md`（条目、回写等）。架构约束：后端无自有时钟（ADR 0001），快捷键的按下 / 松开由全局快捷键插件事件驱动，popup 前端负责数据拉取与回写动作。

## 2. 命令列表

| 命令 | 方向 | 说明 |
| --- | --- | --- |
| `getHotkey` | 前端 → 后端 | 读取已保存的全局快捷键（标准格式字符串）；未设置返回 `null` |
| `setHotkey` | 前端 → 后端 | 设置 / 清除全局快捷键并即时重注册（空串 = 清除） |
| `quickPasteReady` | 前端 → 后端 | popup 前端加载完成握手：若已有挂起的按下事件则补发 `show` |
| `quickPasteClose` | 前端 → 后端 | popup 前端完成回写（或取消）后请求关闭：隐藏窗口、复位状态 |
| `getHotkeyCapability` | 前端 → 后端 | 检测当前环境是否支持全局快捷键（如 Linux Wayland 会话不支持）；`supported=false` 时设置页隐藏录制入口并显示警告（见 5.8） |

事件（后端 → popup 前端）：

| 事件 | 触发时机 | 说明 |
| --- | --- | --- |
| `quick-paste://show` | 快捷键按下 | 小屏已显示；前端自行拉取历史并初始化选中项 |
| `quick-paste://release` | 快捷键松开 | 前端回写选中项并调用 `quickPasteClose` |

## 3. 类型定义

### 快捷键（字符串）

```ts
// 前端视角：tauri-plugin-global-shortcut 标准格式，如 "CommandOrControl+Shift+K"。
// 修饰键 ∈ CommandOrControl | Alt | Shift | Super（可组合，+ 连接）；
// 主键 ∈ A-Z | 0-9 | F1-F12 | Space | Enter | Tab（字母统一小写）。
// 空串表示未设置 / 清除。
type Hotkey = string;
```

```rust
// 后端视角：与 TS 一致，`set_hotkey` 参数即该字符串。
pub struct SetHotkeyReq { pub hotkey: String }
```

### 响应

```ts
type GetHotkeyResp = string | null; // null = 未设置
// setHotkey：成功返回 ()，失败抛 ApiError

// 能力检测（5.8）：supported=false 表示当前环境无法使用全局快捷键
type HotkeyCapabilityResp = { supported: boolean };
```

## 4. 错误码

| 错误码 | 含义 | 中文文案建议 | 英文文案建议 |
| --- | --- | --- | --- |
| `quick_paste.invalid_hotkey` | 格式非法（无主键 / 无修饰键 / 仅 Shift 修饰 / 未知键名） | 快捷键无效：需包含至少一个 Ctrl / Alt / Win 修饰键 | Invalid shortcut: needs at least one of Ctrl / Alt / Win |
| `quick_paste.register_failed` | 全局注册失败（系统占用、插件底层错误） | 快捷键注册失败，可能已被其他程序占用 | Failed to register shortcut (may be taken by another app) |
| `quick_paste.storage_error` | 设置存储读写失败 | 快捷键设置保存失败 | Failed to save shortcut settings |

## 5. 行为说明

### 5.1 快捷键录制（HotkeyRecorder 组件）

- 纯前端组件：展示当前快捷键（标准格式经本地化映射显示，如 `CommandOrControl` → `Ctrl`）；未设置显示「未设置」。
- 点击进入录制态：显示「按下组合键…」；`keydown` 捕获 `Ctrl / Alt / Shift / Super` + 主键组合。
- **纯修饰键按下忽略**（等待主键）；`Esc` 取消录制（不把 Esc 录为快捷键）。
- **校验规则**：必须包含至少一个非 Shift 修饰键（`Ctrl` / `Alt` / `Super`），防止注册裸字母 / 仅 Shift 组合拦截常规输入。
- 录制完成回调携带标准格式字符串，由设置页调用 `setHotkey` 持久化。

### 5.2 全局快捷键生命周期

- **启动**：`setup` 读设置存储（store 文件 `quick-paste.json`，键 `hotkey`）→ 注册；未设置则不注册。
- **setHotkey**：校验（非法 → `quick_paste.invalid_hotkey`）→ 注销旧快捷键 → 注册新快捷键（失败 → `quick_paste.register_failed`，**不持久化**，保持旧注册与旧存储）→ 持久化新值。
- **清除**（空串）：注销当前快捷键 + 存储置空。

### 5.3 按下 / 松开时序

```
用户按住快捷键
  → 插件 Pressed 事件（Rust）
  → 若小屏已激活则忽略；否则：
     pending_show = true
     定位小屏到鼠标光标附近（物理坐标，屏幕边界内 clamp）
     show + set_focus
     若 popup 已 ready 则 emit "quick-paste://show"
  → popup 前端（onMount 已注册监听）：
     先补一次 captureClipboard（主窗口在设置页 / 隐藏期间可能未捕捉到最新复制）
     再拉取 getClipboardHistory → 选中第一项（最新）→ 渲染列表
     首次按下时 WebView 可能未加载完：popup 挂载后 invoke quickPasteReady，
     Rust 发现 pending_show 则补发 show（握手，避免竞态）

用户松开快捷键
  → 插件 Released 事件（Rust）→ emit "quick-paste://release"
  → popup 前端：回写选中项（writeClipboardEntry）→ 完成（含失败）后 invoke quickPasteClose
  → Rust：隐藏小屏、复位激活状态
  → 兜底：Released 后 3 秒小屏仍未关闭则强制隐藏（防前端异常卡住）
```

**小屏数据实时同步（0.2.1 修复）**：

- 剪贴板监听为**应用级**（`src/features/clipboard-history/listener.ts`，App 挂载时启动），不随页面切换或主窗口隐藏停止；捕捉成功广播 `clipboard-history://updated`。
- 小屏**激活期间**收到 `clipboard-history://updated` → 重新拉取历史并保持当前选中条目（被淘汰则重置到第一项）；未激活时不刷新（下次 show 时重新拉取）。
- 小屏 show 时先补一次 `captureClipboard`：兜底主窗口未捕捉到的最新复制（如用户切换语言停留在设置页期间）。
- 后端 `captureClipboard` 带互斥锁：主窗口与小屏并发捕捉同一内容时串行化「读→去重→写」，不会重复插入（第二条按内容指纹去重置顶）。

- **无历史条目**：show 后列表为空，release 时直接关闭、不回写。
- **小屏内按 Esc**：取消，直接 `quickPasteClose`（不回写）。
- **滚轮切换**：`wheel` 的 `deltaY` 决定方向（下滚 +1 / 上滚 -1），索引在 `0..len-1` 内 **clamp（不循环）**；选中变化时列表容器将选中项滚动到可见区域。同时支持键盘 ↑ / ↓（等价操作）。
- **收藏 / 取消收藏（0.2.4）**：小屏内按 `F` 键或在选中条目上点星标按钮，调用 `setEntryFavorite(id, favorited)` 切换收藏状态；变更后广播既有 `clipboard-history://updated` 事件，列表按收藏区置顶重新排列并**保持当前选中条目**（契约 clipboard-history 5.8；收藏条目不纳入数量上限）。
- 小屏显示期间再次按下快捷键（重复 Pressed）忽略；重复 Released 忽略。

### 5.4 小屏窗口

- 预创建于 `tauri.conf.json`（label `quick-paste`）：透明、无边框、置顶、跳过任务栏、初始隐藏、不可缩放、固定尺寸；独立 HTML 入口 `popup.html`（Vite 多入口）。
- 每次 show 定位到鼠标光标附近（复用 `Window::cursor_position` + 当前显示器边界 clamp），**不记忆位置**（window-state 插件 denylist 排除）。
- 回写完成后隐藏而非销毁，下次秒开。
- 主题：复用 `theme.tsx`（localStorage 同源共享），`data-theme` 驱动同一套 CSS 变量。

### 5.5 托盘与关闭行为

- 主窗口 `CloseRequested`：`prevent_close()` + `hide()`（进程常驻，剪贴板监听与定时清理继续——WebView 隐藏后仍存活，ADR 0001 前提不变）。
- 托盘图标：左键单击 / 双击唤出主窗口（`show` + `set_focus` + `unminimize`）；菜单两项——「显示主窗口」「退出」。
- 「退出」：`app.exit(0)`；window-state 插件在窗口关闭流程中保存主窗口位置 / 大小。
- **托盘菜单文案后端硬编码中文**（Rust 侧无 i18n 基建；后续如需国际化再评估）。

### 5.6 窗口状态记忆

- `tauri-plugin-window-state`：自动保存 / 恢复主窗口位置、大小、最大化状态（默认 `StateFlags`）；`denylist` 排除 `quick-paste` 小屏。

### 5.7 与剪贴板历史的关系

- 复用 `getClipboardHistory`（0.2.4 起返回**收藏区在前**、区内按收藏时间倒序，其后按捕捉时间倒序，见契约 clipboard-history 5.8）与 `writeClipboardEntry`（按原始格式回写）两个既有命令，**不新增剪贴板读写逻辑**。
- 回写不触发监听（既有经验，契约 5.5）；若触发则去重置顶自然置顶，无害（实测确认）。
- **小屏收藏（0.2.4）**：popup 内按 `F` 键或点击选中条目星标按钮切换收藏（选中行星标反色为 accent-text：实心 = 已收藏 / 描边 = 未收藏）；变更后 `emit` 既有 `clipboard-history://updated` 事件，主窗口与小屏经既有刷新路径同步（小屏保持当前选中条目）。收藏条目置顶展示、豁免数量上限、不纳入淘汰，见契约 clipboard-history 5.8。

### 5.8 平台能力检测（全局快捷键可用性）

- Linux 下 `tauri-plugin-global-shortcut` 底层 `global-hotkey` 仅实现 **X11 后端**（`XGrabKey`）：**Wayland 会话**中窗口为原生 Wayland，键盘事件不经过 X server，快捷键注册「成功」（XWayland 存在时）但按下**永不触发**。
- `getHotkeyCapability` 返回 `supported`：前端据此决定设置页展示录制入口还是警告（`supported=false` 时**不提供设置**，避免用户配置一个永远不生效的快捷键）。
- 判定逻辑（后端 `service::global_shortcut_supported`，可测纯函数）：
  - 非 Linux 或 X11 会话（`XDG_SESSION_TYPE` 为 `x11`，或缺失且无 `WAYLAND_DISPLAY`）→ `supported = true`；
  - Wayland 会话（`XDG_SESSION_TYPE=wayland`，或缺失但有 `WAYLAND_DISPLAY`）→ 默认 `supported = false`；
  - 例外：`GDK_BACKEND` 显式包含 `x11`（GTK 走 XWayland）时判定为可能生效 → `supported = true`。
- 已知限制：Wayland 下即使 `GDK_BACKEND=x11` 也依赖合成器对 XWayland 抓键的支持，不作为保证；警告文案据此提示用户切换 X11 会话。

## 6. 破坏性影响

- 新功能域，无既有接口破坏。
- 依赖新增（Cargo.toml）：`tauri-plugin-global-shortcut`、`tauri-plugin-window-state`。
- 前端无新 npm 依赖（popup 与主窗口共用既有 `@tauri-apps/api`）。
- capabilities 新增 `quick-paste`（`core:default`）；`tauri.conf.json` 新增 `quick-paste` 窗口；`vite.config.ts` 增加 `popup.html` 多入口。
- 版本递增 `minor` → 0.2.0（三处同步 + CHANGELOG）。
- 0.2.3（三处同步 + CHANGELOG）：新增 `getHotkeyCapability` 命令与设置页平台警告，不破坏既有接口。

## 7. 未决问题

- [ ] 回写是否触发监听：按既有经验不触发，实测确认（见 5.7）。
- [ ] 托盘菜单文案国际化：暂硬编码中文，评估后接入 i18n。
- [ ] `Window::cursor_position` API 可用性：实现时验证；若不可用则小屏固定显示于光标所在显示器中央 / 右下角。
- [ ] 小屏跟随鼠标的物理 / 逻辑像素换算：以光标物理坐标 + 窗口物理尺寸 clamp 到显示器物理边界。
