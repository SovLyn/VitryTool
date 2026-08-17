# 移动端（Android）支持

> 版本 0.2.9｜契约：[docs/api/mobile.md](../api/mobile.md)

## 这是什么

VitryTool 的 **Android 移动端**：定位为「接收 + 转发终端」——手机与桌面在同一局域网时，应用**前台**运行 libp2p 节点接收其他终端的剪贴板广播到**收件箱**，点击条目把内容写入手机剪贴板，再手动粘贴到任意应用。

移动端**不监听**系统剪贴板（Android 无可靠后台监听）、**不广播**本地复制内容、**无后台保活**（首版限制，见待办）。

## 使用方式

- **收件箱页**（底部 tab「收件箱」）：按来源终端分组查看广播内容；**点条目** → 写入手机剪贴板（同时进入本地剪贴板历史）→ 到任意应用粘贴。
- **剪贴板历史页**（底部 tab「剪贴板历史」）：数据源为「从收件箱写剪贴板」的条目（移动端无自动捕捉）；**收藏**功能照常可用。
- **设置页**：语言 / 主题 / 条数上限 / **接收开关** / 终端名 / 在线终端数；**广播开关与快速粘贴（全局快捷键）在移动端隐藏**（功能不存在）。

## 行为要点

- **平台识别**：前端启动时调 `getPlatformInfo`（`isMobile`）隔离桌面功能——不启动剪贴板监听、不下发托盘文案、隐藏快速粘贴/广播设置、files-only 收件箱条目禁用写回。
- **写剪贴板**：统一写**纯文本**（text 优先 → html 剥标签 → 图片元数据写占位文本 `[图片] 名称 (宽x高)`）；仅含文件路径的条目移动端不写（前端禁用 + 兜底错误码 `clipboard.write_unsupported`）。
- **显式入历史**：写剪贴板后直接复用 capture 落盘逻辑把内容记入本地历史（指纹去重置顶/淘汰），不依赖 Android 剪贴板读权限；**不触发广播**（移动端无广播）。
- **节点**：应用进程生命周期内运行（前台接收）；Android mDNS 组播需 `WifiManager.MulticastLock`（MainActivity 持有）；Manifest 含 INTERNET / ACCESS_NETWORK_STATE / ACCESS_WIFI_STATE / CHANGE_WIFI_MULTICAST_STATE。

## 架构（与桌面的差异）

```
桌面：剪贴板监听 → capture → 历史 ──广播──▶ 局域网 ──▶ 手机收件箱 ──点条目──▶ 手机剪贴板 ──▶ 手动粘贴
手机：无监听 / 无广播；回写 = 写剪贴板(clipboard-manager) + 显式入历史（复用 capture 落盘逻辑）
```

- **编译期平台隔离**：`Cargo.toml` target 条件依赖（桌面 4 插件 / 移动 clipboard-manager）；`lib.rs` 按 `#[cfg(desktop)]` / `#[cfg(mobile)]` 注册插件、托盘、quick_paste、窗口事件钩子与命令列表。
- **core/platform.rs**：`getPlatformInfo` 命令 + 剪贴板写分发（`write_text_plain` / `write_text_plain_sync`）+ 移动端可写文本提取（`mobile_writable_text` / `strip_html`）+ 全局快捷键能力判定（0.2.3 迁入，core 自包含）。
- **功能解耦**：lan-sync 移动端回写经 `core::hooks::mobile_clipboard_write` 通道，由 clipboard_history 在 setup 注册实现（写剪贴板 + 显式入历史）。
- **capabilities**：桌面 `default.json`（clipboard-x）与移动 `mobile.json`（clipboard-manager）按 `platforms` 字段分平台生效。

## 已知限制（README 同步）

- **无后台保活**：应用退到后台，节点可能被系统回收 → 收不到广播；回到前台自动恢复（进程被杀则重启应用）。
- **无剪贴板监听**：手机本地复制的内容不会进入历史 / 不会广播；历史页只有「从收件箱写剪贴板」的记录。
- **仅文本写入**：图片 / 文件字节无法写入手机剪贴板（图片仅元数据占位文本，文件路径条目禁用写回）。
- **mDNS 依赖 WiFi**：纯蜂窝网络下无组播，无法发现终端。

## 待办（见 TODO.md）

后台保活（前台服务 + 常驻通知）、图片/文件字节写入、移动端广播（需监听替代方案）、iOS 支持、Android release 签名（CD secrets 配置，见 release.yml）。
