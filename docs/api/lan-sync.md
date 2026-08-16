# 接口契约文档：lan-sync（局域网剪贴板同步）

- 状态：`已实现`（0.2.5）
- 关联功能文档：[docs/features/lan-sync.md](../features/lan-sync.md)
- 版本影响：`patch`（0.2.4 → 0.2.5；与近期功能实践一致，见 `docs/versioning.md` 注）
- 调研与设计决策来源：`dev/interface-drafts/lan-sync-research.md`（可行性调研）、`dev/interface-drafts/lan-sync-contract-draft.md`（决策定稿）

## 1. 概述

局域网内运行本应用的终端，通过 libp2p（mDNS 发现 + TCP/QUIC 连接 + gossipsub 广播）互联：本机复制产生新历史条目时自动广播，其他终端将收到的内容放入「收件箱」（按来源节点分桶，每桶最新 8 条、全局最多 8 个节点），不自动写回系统剪贴板（避免打断用户）。广播 / 接收可分别开关。

- 节点身份：持久化 ed25519 keypair（AppData/`peer-key.json`），**peerId 即身份，不依赖 IP**（局域网 IP 可变）。
- 单实例：应用强制单实例（tauri-plugin-single-instance），一台机器一个节点。
- 跨端协议：固定 gossipsub 主题 `vitrytool-lan-clipboard`，信封 `v` 字段与项目版本同步（本次 `0.2.5`），**向后兼容：只增字段，接收端忽略未知字段**。

## 2. 命令列表

| 命令 | 方向 | 说明 |
| --- | --- | --- |
| `getLanSyncStatus` | 前端 → 后端 | 节点状态：peerId、终端名、广播/接收开关、节点运行状态、发现终端数 |
| `setLanSyncBroadcast(enabled)` | 前端 → 后端 | 开/关广播（关 = 不再发布，节点仍运行、仍接收） |
| `setLanSyncReceive(enabled)` | 前端 → 后端 | 开/关接收（关 = 收件箱不入新内容；仍公告自身） |
| `setLanSyncTerminalName(name)` | 前端 → 后端 | 设置终端名（持久化，随广播信封下发） |
| `getLanInbox()` | 前端 → 后端 | 收件箱全量（按节点分组，组按最新条目时间倒序） |
| `writeLanInboxEntry(id)` | 前端 → 后端 | 回写：按原格式写系统剪贴板 → 本机 capture 进历史（防环不重广播） |
| `deleteLanInboxEntry(id)` | 前端 → 后端 | 单条删除 |
| `clearLanInbox()` | 前端 → 后端 | 清空收件箱 |

事件（后端 → 前端）：

| 事件 | 载荷 | 说明 |
| --- | --- | --- |
| `lan-sync://inbox-updated` | `{ reason: "received" \| "deleted" \| "cleared", id?: string }` | 收件箱变化（新消息 / 删除 / 清空）时通知刷新 |

## 3. 类型定义

### 响应（后端 → 前端）

```ts
// LanSyncStatus（getLanSyncStatus）
interface LanSyncStatus {
  peerId: string;          // 本机节点身份（完整 peerId）
  terminalName: string;    // 终端名（默认主机名）
  broadcastEnabled: boolean;
  receiveEnabled: boolean;
  nodeRunning: boolean;    // 节点是否在运行（启动失败等场景）
  peerCount: number;       // 当前已连接/发现的终端数
}

// LanInboxEntry（收件箱条目）
interface LanInboxEntry {
  id: string;              // 本机 uuid
  peerId: string;          // 来源节点
  terminalName: string;    // 来源终端名（发送时快照）
  receivedAt: string;      // 本机接收时间 ISO8601（组内排序键）
  sentAt: string;          // 发送方时间 ISO8601（展示用）
  text?: string;
  html?: string;
  rtf?: string;
  filePaths?: string[];    // 文件路径（首版按文本广播，跨机可能无效）
  imageMeta?: { name: string; width?: number; height?: number; size?: number }; // 图片仅元数据（字节传输 TODO）
  fingerprint: string;     // 去重键（与本地历史同款指纹规则）
}

// LanInboxNode（收件箱分组：一个来源节点一桶）
interface LanInboxNode {
  peerId: string;
  terminalName: string;    // 组内最新条目的终端名（快照可能变化）
  entries: LanInboxEntry[]; // 按 receivedAt 倒序，最多 8 条
}

// getLanInbox 响应
interface LanInboxResp {
  nodes: LanInboxNode[];   // 按组内最新条目时间倒序
}
```

```rust
// Rust（serde，字段与上方 TS 一一对应，camelCase）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanSyncStatus { peer_id: String, terminal_name: String, broadcast_enabled: bool, receive_enabled: bool, node_running: bool, peer_count: usize }
// …（其余类型同构，见 src-tauri/src/features/lan_sync/service.rs）
```

## 4. 错误码

| 错误码 | 含义 | 中文文案建议 | 英文文案建议 |
| --- | --- | --- | --- |
| `lan.storage_error` | 收件箱/设置持久化失败 | 收件箱存储失败，请重试 | Inbox storage error, please retry |
| `lan.entry_not_found` | 条目不存在（已删除/已淘汰） | 该条目不存在或已被淘汰 | Entry not found or already evicted |
| `lan.invalid_name` | 终端名非法（空 / 超长） | 终端名不能为空且不超过 32 字符 | Terminal name must be non-empty and ≤ 32 chars |
| `lan.node_not_running` | 节点未运行（广播/收件箱操作失败） | 同步节点未运行，请重启应用 | Sync node is not running, please restart |
| `lan.peer_node_error` | 节点层错误（身份/网络内部错误） | 同步节点异常，请查看日志 | Sync node error, check logs |
| `lan.too_large` | 内容超 1MiB 上限（广播侧内部跳过，命令一般不暴露） | 内容过大，未广播 | Content too large to broadcast |

## 5. 行为说明

### 5.1 节点生命周期与身份

- 节点在应用 setup 阶段启动（独立 tokio runtime 后台线程），应用退出时停止；生命周期 = 应用生命周期（托盘常驻期间持续运行）。
- 身份：首次启动生成 ed25519 keypair 持久化于 `AppData/peer-key.json`；文件缺失/损坏时重新生成并记录日志。终端名默认取主机名，持久化于 `AppData/lan-sync.json`（键 `terminalName`）。
- 单实例：第二实例启动时自动退出并唤出主窗口（tauri-plugin-single-instance）。

### 5.2 广播（发送）

- 触发点：剪贴板历史 `capture_clipboard` 产生**新条目**（`is_new == true`）后，经 `core` 广播钩子通知 lan-sync。
- 条件（全部满足才发布）：广播开关开、节点运行中、内容指纹不在「近期接收集合」（防环，见 5.4）、序列化后 ≤ 1MiB（超限静默跳过，仅日志）。
- 载荷：信封 JSON（见 5.6），携带 entry 的 text / html / rtf / filePaths / imageMeta（图片仅元数据；图片字节传输 TODO）。
- 回写（write_clipboard_entry / writeLanInboxEntry）不触发广播（回写不产生 `is_new`，或命中防环）。

### 5.3 接收与收件箱

- 收到信封：`peerId == 本机` 跳过（自己的广播不入箱）；接收开关关 → 忽略（不入箱）。
- 指纹去重：命中收件箱内既有条目 → 刷新该桶内位置（置顶），不新增。
- 容量：按来源节点分桶，每桶最新 **8 条**；全局最多 **8 个节点桶**——第 9 个节点来消息时，淘汰「桶内最新条目时间最旧」的节点整桶，再入新节点。
- 排序：桶内按 `receivedAt` 倒序（本机接收时间，不用发送方时钟）；节点桶按桶内最新 `receivedAt` 倒序。
- 每次收件箱变化（新消息 / 删除 / 清空）emit `lan-sync://inbox-updated`。
- 持久化：收件箱跨重启保留（`AppData/lan-inbox.json`）。

### 5.4 防环

- 后端维护「近期接收内容指纹 LRU 集合」（上限 100 条）；收到广播时记录指纹。
- `capture_clipboard` 产生新条目时，指纹命中该集合 → 跳过广播（防「回写 → capture → 再广播」回环）；未命中 → 正常广播（并随 5.2 流程发布）。

### 5.5 回写

- `writeLanInboxEntry(id)`：按 html → rtf → text → files → imageMeta 优先级写系统剪贴板（与 `write_clipboard_entry` 同语义；imageMeta 无字节，写为占位文本 `[图片] 名称 (宽x高)`）。
- 写剪贴板会触发本机 capture 监听 → 内容进入本地历史（去重置顶）；因指纹命中近期接收集合，**不会**再广播回网络。

### 5.6 跨端协议信封（v=0.2.5）

```json
{
  "v": "0.2.5",
  "ts": 1786713054000,
  "peerId": "12D3Koo...",
  "terminal": "SOVLYN",
  "kinds": ["text", "html"],
  "text": "…",
  "html": "<p>…</p>",
  "rtf": "{\\rtf1 …}",
  "filePaths": ["C:\\…"],
  "imageMeta": { "name": "hash.png", "width": 1920, "height": 1080, "size": 102400 }
}
```

- 固定主题 `vitrytool-lan-clipboard`；gossipsub 自带消息去重（msg id = 内容哈希）。
- **兼容约束**：`v` 只增不改；接收端解析已知字段、忽略未知字段；`kinds` 为声明清单，接收端按字段存在与否处理（不依赖 kinds 强校验）。旧版终端（同主题、旧 v）收到新版信封：可解析部分照常展示，未知字段忽略。

### 5.7 开关与设置

- 广播/接收开关默认**全开**，持久化于 `AppData/lan-sync.json`。
- 关广播：不再发布新内容，节点仍运行、仍接收、仍公告自身。
- 关接收：收件箱不再入新内容（已有内容保留），节点仍公告自身（其他终端可见）。
- 开关切换不重启节点。
- **入口（0.2.7）**：除设置页外，系统托盘菜单提供「剪贴板广播」「剪贴板接收」两个可勾选项（CheckMenuItem）快速开关——经 `core::hooks` 注册的开关钩子读写（与 `setLanSyncBroadcast` / `setLanSyncReceive` 命令同一共享态与持久化路径），勾选态随设置实时同步；文案由前端 i18n 经 `setTrayLabels` 下发（契约 quick-paste 5.5）。
- **设置变化事件（0.2.7）**：托盘或设置页切换广播/接收后，后端 emit `lan-sync://settings-updated`（载荷含对应字段），设置页监听并重新拉取 `getLanSyncStatus` 实时刷新开关状态。

## 6. 破坏性影响

- 无：全新功能，不动既有命令/事件/存储。
- 唯一内部改动：`capture_clipboard` 在产生新条目后调用 `core` 广播钩子（可选通道，默认空实现，不影响既有行为）；`Cargo.toml` 新增依赖（libp2p、tokio、tauri-plugin-single-instance）。
- 0.2.5 起 `core::hooks` 新增 lan-sync 开关钩子（`register_lan_sync_switches`，未注册时托盘读写为空操作）。
- 版本 0.2.5 三处同步（Cargo.toml / tauri.conf.json / package.json）。

## 7. 未决问题（转 TODO，不进首版）

- 图片/文件字节传输（含 gossipsub 分片）；接收器模式（立即写回且无法发送）；黑名单；收件箱收藏/跨端转发；开关快捷键；手动添加对端；libp2p-mdns Windows 虚拟网卡组播出口根治；重置身份入口。（「全局错误通知 UI」已于 0.2.8 完成，见 `docs/api/notify.md`。）
