# 接口契约文档：mobile（移动端支持 / Android）

- 状态：`已实现`（0.2.9；真机验证项见第 7 节）
- 版本影响：`patch`（0.2.8 → 0.2.9；与近期功能实践一致，见 `docs/versioning.md` 注）
- 关联文档：桌面端各功能契约（clipboard-history / lan-sync / quick-paste / notify）——本文件只定义**平台差异**，未提及处两平台行为一致

## 1. 概述

0.2.9 增加 **Android** 移动端支持。移动端定位为「接收 + 转发终端」：

- 应用**前台**运行时，libp2p 节点接收局域网广播 → 内容进入收件箱（与桌面同一协议、同一信封）；
- 用户点击收件箱条目 → 写入**系统剪贴板**（`tauri-plugin-clipboard-manager`，写纯文本）→ 手动粘贴到任意应用；
- 移动端**不监听**系统剪贴板（Android 无可靠后台剪贴板监听）、**不广播**本地复制内容、**无后台保活**（前台服务不在首版，转 TODO）。

## 2. 命令列表

### 新增

| 命令 | 方向 | 说明 |
| --- | --- | --- |
| `getPlatformInfo` | 前端 → 后端 | 平台识别：`isMobile` / `platform` / 全局快捷键能力。前端功能隔离的唯一依据（见 5.1）。合并 0.2.3 `getHotkeyCapability` 的能力，旧命令保留（内部委托，见 6） |

### 既有命令的平台差异（命令面不变，后端内部按平台分发）

| 命令 | 桌面（现状） | 移动端 |
| --- | --- | --- |
| `writeClipboardEntry(id)` | clipboard-x 按原格式写回；系统监听异步 capture 置顶 | clipboard-manager 写**纯文本**（策略见 5.2）；**显式置顶**（同指纹不新增，见 5.3） |
| `writeLanInboxEntry(id)` | clipboard-x 按原格式写回 → 监听 capture 进历史 | clipboard-manager 写纯文本；**显式入历史**（复用 capture 落盘逻辑，见 5.3）；**不广播** |
| `getLanSyncStatus` / `setLanSyncReceive` / `setLanSyncTerminalName` / 收件箱四命令 | 同 | **无差异**（移动端核心链路） |
| `setLanSyncBroadcast(enabled)` | 开/关广播 | 命令仍注册但**前端无入口**（移动端无广播实现；见 5.4） |
| quick_paste 全部命令 / `setTrayLabels` / `captureClipboard` / `cleanupOrphanImages` | 同 | **移动端不注册**（不编译，前端无入口；见 5.1 隔离矩阵） |

## 3. 类型定义

### 响应（后端 → 前端）

```ts
// PlatformInfo（getPlatformInfo）
interface PlatformInfo {
  isMobile: boolean;                    // true = android / ios
  platform: "windows" | "macos" | "linux" | "android" | "ios";
  hotkeyCapability: {
    supported: boolean;                 // 移动端恒为 false（无全局快捷键概念）
  };
}
```

```rust
// Rust（serde，字段与上方 TS 一一对应，camelCase）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub is_mobile: bool,
    pub platform: String, // "windows" | "macos" | "linux" | "android" | "ios"
    pub hotkey_capability: HotkeyCapabilityResp, // 复用 quick_paste 的响应结构
}
```

## 4. 错误码

| 错误码 | 含义 | 中文文案建议 | 英文文案建议 |
| --- | --- | --- | --- |
| `clipboard.write_unsupported` | **仅移动端**：条目内容无法写入剪贴板（如仅含文件路径） | 该内容格式在移动端不支持写入 | This content format cannot be written on mobile |

- 仅作后端**兜底**：前端按 5.2 的字段判断先禁用入口，正常路径不触发。
- 增量新增，不改变既有错误码语义。

## 5. 行为说明

### 5.1 平台识别与前端隔离矩阵

- 前端启动时调用一次 `getPlatformInfo`，以 `isMobile` 隔离以下功能（桌面行为零改动）：
  - 剪贴板监听：`startClipboardCapture` 移动端**不启动**（含 5 分钟孤儿图片定时清理）；
  - 托盘文案：`setTrayLabels` 移动端**不调用**（无托盘）；
  - 设置页：隐藏「快速粘贴」整组（录制器 + Wayland 警告）、隐藏「广播」开关；保留语言/主题/条数上限/接收开关/终端名/peer 信息；
  - 收件箱：files-only 条目写回按钮**禁用**并提示（新 i18n 键 `inbox.writeUnsupported`）。
- **历史页在移动端保留**：数据源为「从收件箱写剪贴板」的条目（见 5.3），收藏功能照常可用。

### 5.2 移动端写剪贴板策略（统一纯文本）

- 优先级：条目有 `text` → 写 text；无 text 有 `html` → **后端剥 HTML 标签**得纯文本后写入；只有 `imageMeta` → 写占位文本 `[图片] 名称 (宽x高)`（与桌面同语义）；**仅含 `filePaths` → 不写**。
- 原因：`tauri-plugin-clipboard-manager` 在 Android 只保证写纯文本（HTML 写支持待真机验证，策略不依赖它）；手机端粘贴进任意应用以文本为主。
- 实现形态：后端新增可测纯函数（剥 HTML / 提取移动端可写文本），`write_clipboard_entry` 与 `write_lan_inbox_entry` 的移动端分支调用；desktop 分支保持原逻辑（不动）。

### 5.3 移动端显式入历史 / 置顶

桌面端写回靠「系统监听 → capture」自动进历史；移动端无监听，改为**显式调用 capture 的落盘逻辑**（内容已在内存，不需要 Android 剪贴板读权限）：

- `writeLanInboxEntry(id)`（移动端）：读收件箱条目 → 写剪贴板（5.2 文本）→ **显式插入本地历史**（复用 capture service 的指纹去重 / 去重置顶 / 即时淘汰）→ 不触发广播（防环集合不涉及，移动端本就不发布）。
- `writeClipboardEntry(id)`（移动端）：从历史读条目 → 写剪贴板（5.2 文本）→ 同指纹**显式置顶**（不新增重复条目），与桌面「写回即置顶」行为一致。
- 历史条目落盘格式：仅 text（+ 可选 html 剥文本），无图片/文件字段；移动端历史页预览/收藏/删除/条数上限照常工作。

### 5.4 移动端 lan-sync 节点

- 生命周期 = **应用进程生命周期**（无前台服务）：activity 在前台进程存活 → 可接收；退后台短时可能仍收；被系统回收后重启应用恢复。v1 接受此限制（转 TODO：前台服务保活）。
- mDNS 组播：Android 需 `WifiManager.MulticastLock` 才能收到组播包 → 在 `gen/android` 的 MainActivity（Kotlin）获取并持有锁；Manifest 权限：`INTERNET` / `ACCESS_NETWORK_STATE` / `ACCESS_WIFI_STATE` / `CHANGE_WIFI_MULTICAST_STATE`。
- 节点仍**公告自身**（其他终端可见手机在线，peerCount 含手机）；但**不发布**任何广播（广播触发点为 capture，移动端无 capture）。
- 单实例插件（tauri-plugin-single-instance）仅桌面注册；Android 无第二实例概念。

### 5.5 桌面回归保证

- 平台隔离全部为**编译期**：`Cargo.toml` 的 target 条件依赖 + `lib.rs` 的 `#[cfg(desktop)]` / `#[cfg(mobile)]`（tauri 提供的 cfg alias）。
- 桌面构建产物与 0.2.8 行为完全一致（依赖集合、命令面、插件注册不变）；移动端才引入 clipboard-manager。

## 6. 破坏性影响

- `Cargo.toml`：`tauri-plugin-clipboard-x` / `tauri-plugin-global-shortcut` / `tauri-plugin-window-state` / `tauri-plugin-single-instance` 移入 `[target.'cfg(not(any(target_os = "android", target_os = "ios")))'.dependencies]`；新增 `[target.'cfg(any(target_os = "android", target_os = "ios"))'.dependencies]` 的 `tauri-plugin-clipboard-manager`。桌面解析出的依赖集合不变。
- `lib.rs`：插件注册 / 托盘 init / quick_paste init / 窗口事件钩子加 cfg 门控；lan_sync init 两平台均执行。
- `getHotkeyCapability`：**保留**，内部委托 `getPlatformInfo` 的能力字段（前端旧调用不受影响）。
- 新增错误码 `clipboard.write_unsupported`（增量）。
- 版本 0.2.9 三处同步（Cargo.toml / tauri.conf.json / package.json）。

## 7. 未决问题 / 验证项（转 TODO，不进首版或待真机验证）

- 后台保活（Android 前台服务 + 常驻通知）——TODO；
- 图片/文件字节写入移动端剪贴板——TODO（v1 仅文本）；
- 移动端广播本地复制内容（需监听替代方案：如 WebView 聚焦时轮询）——TODO；
- 真机验证项：mDNS 接收（MulticastLock 生效）、写剪贴板、`tray-icon` feature 在 Android 编译、clipboard-manager 写 HTML（策略已规避，仍记录）；
- iOS 支持——本次明确不做。
