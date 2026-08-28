# 接口契约文档：lan-file（局域网文件共享）

- 状态：`已实现`（0.3.0，2026-08-28；协议层真机验证通过——双机 VLF 握手互通、伪造身份拒绝、peerId 绑定独立复核；UI 完整互通待用户人工实测）
- 关联功能文档：[docs/features/lan-file.md](../features/lan-file.md)
- 版本影响：`minor`（0.2.9 → 0.3.0，新功能）
- 调研来源：`C:\Users\SovLy\Documents\rust\lan_file_test\五模型局域网文件传输预研评测报告.md`（五方案横评，推荐蓝本 §7.3）
- 商讨记录：后端 Q1–Q12 + 前端 F1–F7（/grill 会话，2026-08-20；复审修订 A–D 同日确认）

## 1. 概述

局域网内运行本应用的桌面终端之间**点对点推送文件**：用户在「文件」页选择本地文件 → 点选在线终端发起 → 对端弹出提议面板，**确认后**经独立裸 TCP 传输（多文件任务内串行），落盘前全文件 SHA-256 对账 + 原子 rename。核心规则：

- **仅确认后传输**（TOFU）：首次收到某终端的提议需用户接受；接受即信任（记 peerId——**peerId 即该终端 ed25519 身份的 multihash，身份即指纹**，不存在「同一 peerId 换公钥」的冒充空间），后续免确认。**陌生 peerId 撞已信任终端名 = 红色警告卡**（改名仿冒 / 对端重置身份顶旧名回来）。用户显式取消 = 永久终止，**不再续传**。
- **断线自动续传**：异常中断（网络抖动 / 对端瞬断）在 120s 宽限窗口内重连自动从已收偏移续传，用户无感；窗口超时按失败清理。
- **单会话**：同一时刻一个活跃交互传输任务（任务内多文件串行，无大小上限，落盘前磁盘检查）。不做传输历史列表（v1 决策）；本会话内完成卡折叠摘要。
- **自动图片通道**（与 lan-sync 联动，§5.7）：已信任终端复制截图 → 字节自动送达对端收件箱图片目录，收件箱条目从 `[图片]` 占位点亮为真实图片。免确认（受严格门控）。
- **移动端**（§5.9）：桌面专属功能 + 移动端仅自动图片**接收**例外（无 UI 命令面）。
- 发现/信令复用 `core::peer_node`（libp2p mDNS + gossipsub 公告）；数据面**独立裸 TCP**（不经 libp2p 流），规避 libp2p 数据面在 Windows 多网卡/坏链路的实测缺陷（预研报告 §1.2、§4.5）。

## 2. 命令列表

命令按平台注册：`sendLanFile` / `acceptLanFile` / `rejectLanFile` / `cancelLanFileTransfer` 仅桌面；`getLanFileStatus` / `getLanFilePeers` / `setLanFileEnabled` 桌面注册（移动端不注册任何 lan-file 命令，§5.9）。

| 命令 | 方向 | 说明 |
| --- | --- | --- |
| `getLanFileStatus` | 前端 → 后端 | 总开关、监听状态与端口、公告可用终端数、已信任终端数 |
| `getLanFilePeers` | 前端 → 后端 | 可传终端列表（公告了 lan-file 能力的在线节点，含信任标记） |
| `sendLanFile(peerId, filePaths)` | 前端 → 后端 | 发起传输任务 → 返回 `transferId`；已有活跃任务报 `lan_file.busy` |
| `acceptLanFile(transferId)` | 前端 → 后端 | 接受提议（未知终端同时写入信任表 = TOFU 确认） |
| `rejectLanFile(transferId)` | 前端 → 后端 | 拒绝提议（通知对端 + 清理，无状态残留） |
| `cancelLanFileTransfer(transferId)` | 前端 → 后端 | **显式取消 = 永久终止**：通知对端、删 `.tmp` 与 sidecar、写取消墓碑（§5.5） |
| `setLanFileEnabled(enabled)` | 前端 → 后端 | 功能总开关（默认开；关闭 = 停监听、停公告、终止活跃任务、图片通道停） |

事件（后端 → 前端）：

| 事件 | 载荷 | 说明 |
| --- | --- | --- |
| `lan-file://peers-updated` | 无（收到后重拉 `getLanFilePeers`） | 公告终端上下线 |
| `lan-file://incoming` | `LanFileOffer` | 新提议到达（§3；resumed 续传**不发**此事件，静默恢复） |
| `lan-file://transfer-updated` | `LanFileTransfer`（活跃任务全量快照，§3） | 状态/进度变化，进度更新节流 ≤4 次/秒；`transferId == null` 表示回到空闲 |
| `lan-file://settings-updated` | `{ fileShare: boolean }` | 总开关变化（托盘 ⇄ 设置页双向同步，同 `lan-sync://settings-updated` 模式） |

自动图片通道**不发任何事件**（静默降级原则，§5.7）；其完成反馈由收件箱既有的 `lan-sync://inbox-updated` 点亮路径体现（前端按 hash 轮询/懒加载缩略图，契约 lan-sync 增量）。

## 3. 类型定义

### 响应（后端 → 前端）

```ts
// LanFileStatus（getLanFileStatus）
interface LanFileStatus {
  enabled: boolean;          // 总开关
  listening: boolean;        // TCP 监听是否在运行（enabled 且启动成功）
  tcpPort: number | null;    // 当前监听端口（OS 动态分配；未监听为 null）
  peerCount: number;         // 公告了 lan-file 能力的在线终端数
  trustedCount: number;      // 已信任终端数（TOFU 表）
}

// LanFilePeer（getLanFilePeers 元素）
interface LanFilePeer {
  peerId: string;            // libp2p 身份（与 lan-sync 同一 peerId；即指纹，multihash(ed25519 公钥)）
  terminalName: string;      // 公告时快照
  fingerprint: string;       // "SHA256:" + base64(ed25519 公钥)，展示参考用
  trusted: boolean;          // peerId ∈ TOFU 信任表
  supportsInteractive: boolean; // 对端支持人工确认传输（桌面 true / 移动 false）
}

// getLanFilePeers 响应
interface LanFilePeersResp { peers: LanFilePeer[] }

// LanFileOffer（lan-file://incoming 载荷）
interface LanFileOffer {
  transferId: string;        // 发起方生成，全生命周期稳定（含续传重试）
  peerId: string;            // 来源终端
  terminalName: string;
  fingerprint: string;       // 来源公钥指纹（TOFU 展示参考信息）
  nameClash: boolean;        // true = peerId 陌生但 terminalName 与已信任终端撞名（红色警告卡场景，见 5.4；判定在端侧做，前端无信任表）
  files: { name: string; size: number }[]; // 净化后展示名
  totalBytes: number;
  knownFromMinutes: number;  // 该 peerId 上次出现在公告列表距今（TOFU 参考信息）
}

// LanFileTransfer（lan-file://transfer-updated 载荷）
interface LanFileTransfer {
  transferId: string | null; // null = 空闲
  direction: "send" | "receive";
  peerId: string;
  terminalName: string;
  state: "offering" | "transferring" | "resuming" | "done" | "failed" | "cancelled" | "rejected";
  files: {
    name: string;
    size: number;
    transferredBytes: number;
    status: "pending" | "transferring" | "done";
  }[];
  totalBytes: number;
  transferredBytes: number;
  bytesPerSec: number;       // 后端 EMA（指数滑动平均，窗口 ~3s；resuming 期不刷新）
  savedPaths?: string[];     // 仅接收侧 done：落盘最终路径（「打开所在位置」用）
  error?: { code: string; params?: Record<string, string | number> }; // 稳定错误码，前端 i18n 翻译
}

// sendLanFile 响应
interface SendLanFileResp { transferId: string }
```

```rust
// Rust（serde，字段与上方 TS 一一对应，camelCase）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanFileStatus {
    pub enabled: bool, pub listening: bool, pub tcp_port: Option<u16>,
    pub peer_count: usize, pub trusted_count: usize,
}
// …（其余类型同构，实现见 src-tauri/src/features/lan_file/service.rs）
```

> **实现注记（0.3.0 已实现，2026-08-28）**：`transfer-updated` 空闲快照以「省略
> `transferId` 字段」表达 null 语义（`#[serde(skip_serializing_if)]`）；`LanFilePeer`
> / `LanFileOffer` 增加 `fingerprint` 之外的 `terminalName` 快照与
> `knownFromMinutes`（公告 age 取整分钟）；错误码 15 个与 §4 完全一致。

## 4. 错误码

| 错误码 | 含义 | 中文文案建议 | 英文文案建议 |
| --- | --- | --- | --- |
| `lan_file.busy` | 已有活跃传输任务 | 已有传输进行中，请等待完成或取消 | A transfer is already in progress |
| `lan_file.not_enabled` | 功能总开关关闭 | 文件共享已关闭 | File sharing is turned off |
| `lan_file.peer_unsupported` | 目标终端不支持人工文件传输（移动端/未公告端口/旧版） | 该终端不支持文件接收 | This device cannot receive files |
| `lan_file.peer_not_found` | 目标终端不在线（公告已过期） | 终端不在线，请确认对方已开启文件共享 | Device is offline |
| `lan_file.file_not_found` | 发送侧文件不存在（发起后消失） | 文件不存在或已被移动 | File not found or has been moved |
| `lan_file.file_unreadable` | 发送侧文件读不了（权限/锁文件） | 文件无法读取 | File cannot be read |
| `lan_file.invalid_path` | 入参路径非法（空列表/目录/超长） | 文件路径无效 | Invalid file path |
| `lan_file.disk_full` | 接收侧磁盘剩余空间不足（总大小 + 200MB 余量） | 磁盘空间不足 | Not enough disk space |
| `lan_file.offer_timeout` | 提议 60s 无人响应（自动拒绝） | 对方未在 60 秒内接受传输 | The request was not accepted within 60 seconds |
| `lan_file.rejected` | 对端拒绝了传输 | 对方拒绝了传输 | The request was declined |
| `lan_file.integrity_mismatch` | 全文件哈希对账失败 | 文件校验失败，已丢弃 | Integrity check failed, files discarded |
| `lan_file.transfer_failed` | 数据面 IO 失败 / 断线超窗 / 对端中断 | 传输中断 | Transfer interrupted |
| `lan_file.cancelled` | 对端显式取消（本地发起 notify） | 对方取消了传输 | The transfer was cancelled by the peer |
| `lan_file.storage_error` | 设置/信任表/sidecar 持久化失败 | 文件共享设置保存失败 | Failed to save file sharing settings |
| `lan_file.node_not_running` | peer_node 未运行（公告/监听不可用） | 同步节点未运行，请重启应用 | Sync node is not running, please restart |

## 5. 行为说明

### 5.1 开关、设置与托盘

- 总开关默认**开**；持久化 `AppData/lan-file.json`（键：`enabled`、`trustedPeers[]`：`{peerId, terminalName, trustedAt}`——peerId 即身份，无需另存公钥）。
- 关闭：停止 TCP 监听、停止 gossipsub 公告、活跃交互任务按用户取消语义终止、图片通道停；节点本体（lan-sync 用途）不受影响。
- **托盘快速开关**（复用 0.2.7 hooks 模式）：CheckMenuItem「文件共享」，与设置页经 `lan-file://settings-updated` 双向同步；文案由前端 i18n 经 `setTrayLabels` 下发（**契约 quick-paste 5.5 增量**：新增可选参数 `fileShare`，旧调用不破）。
- 开关切换不重启 peer_node。

### 5.2 发现与公告（复用 peer_node）

- **不改 mDNS**：终端在线/离线判定沿用 peer_node 的 libp2p mDNS 发现（0.2.5 已上线的链路）。多网卡组播坑沿用现状（已知限制，README 注明；根治仍为 TODO）。
- **`core::peer_node` 增量为多主题**：`NodeCommand::Publish { topic, data }` / `NodeEvent::PubsubMessage { topic, source, data }`（纯通道字段，业务语义仍留在各 feature；lan_sync 传剪贴板主题，行为不变）。
- **公告主题** `vitrytool-lan-file-announce`，负载：

```json
{ "v": "0.3.0", "peerId": "12D3Koo...", "terminal": "SOVLYN",
  "tcpPort": 52341, "fingerprint": "SHA256:base64…", "caps": ["file", "img"] }
```

  - `caps`：`file` = 支持人工确认传输（桌面），`img` = 支持图片通道接收（桌面/移动）。移动端仅 `["img"]`。
  - 时机：启动即公告 + 经 mDNS 发现新对端后补公告（等 gossipsub 订阅握手，复用 0.2.5 实测经验）+ 每 5 分钟周期重公告；关闭开关/退出时尽力发撤销（发不出则靠接收侧 TTL）。
  - 接收侧对公告项**过期驱逐**：12 分钟未见重公告 → 从 peers 移除（与周期公告 2 倍冗余）。
  - `fingerprint` 为展示参考（peerId 本身已密码学绑定公钥，公告中的公钥可校验 `multihash(公钥)==peerId`，不一致即丢弃该公告并记日志——防手滑伪造，非新信任机制）。
- **兼容**：只增字段；旧版终端收到公告因未订阅该主题而天然无视；本端对解析失败的公告仅记日志。
- **TCP 监听**：监听 `0.0.0.0:0`（OS 动态端口），端口随公告下发；重启换端口可接受（peerId 是身份，IP:port 不是）。连接建立超时 5s。Windows 防火墙可能拦入站（预研 §7.2）→ 已知限制，README 注明首启放行指引。

### 5.3 会话模型（单活跃交互会话 + 图片小通道）

- **交互会话**：全局同一至多 1 个活跃交互传输（发起方或接收方任一角色，至多 1 个）。占用中再次 `sendLanFile` → `lan_file.busy`；作为接收方收到**第二个**新提议（非续传）→ 自动拒绝 `busy` + 本端 notify warning。
- **状态机**（`LanFileTransfer.state`）：

```
offering ─accept→ transferring ─全部文件校验通过→ done
   │ │                │  ╲（网络断，≤120s 重连）→ resuming → transferring
   │ │                │   ╲（超窗 / IO 错误 / 对端取消）→ failed / cancelled
   │ ├─reject→ rejected（本侧映射为 cancelled 展示）
   │ └─60s 无响应→ failed(offer_timeout)
（任何状态）用户 cancel → cancelled（永久终止，见 5.5 墓碑）
```

- **图片通道不占交互会话槽**：独立小队列（串行处理，队列深度上限 8，超出丢弃最旧并记日志——截图小文件，丢弃无损，收件箱占位照常）。
- `bytesPerSec`：仅 transferring 态刷新（EMA α≈0.3/次更新）。

### 5.4 提议、TOFU 与信任

- 提议到达（`lan-file://incoming`）：主窗可见 → 前端顶部面板呈现；隐藏 → 前端唤窗（**契约只定事件**，展示策略归前端 F2）。**60s** 无人 accept/reject → 自动拒绝（`offer_timeout`，双向清理）。
- `acceptLanFile(transferId)`：若 peerId 不在信任表 → **同时写入**（peerId + 当时终端名 + trustedAt）= TOFU 确认动作；已在表内则纯放行。
- **撞名警告（红色警告卡的真实触发器，复审修订 A）**：libp2p 的 peerId 是 ed25519 公钥的 multihash——换密钥必然换 peerId，「同 peerId 不同公钥」在协议上不可能，故旧设想的「指纹变化告警」是死分支。真实可检测且值得警告的场景是**陌生 peerId 顶撞已信任终端名**（改名仿冒 / 对端重置 `peer-key.json` 后顶着旧名字回来）。判定在端侧（前端无信任表）：提议载荷 `nameClash = true` → 前端渲染红色警告卡（「这不是你之前信任的『X』（身份已变化）」，默认按钮位权移到「拒绝」）；此时接受 = 正常写入信任表（表按 peerId 键控，同名两条记录并存，旧记录可另行移除）；拒绝即终止。`fingerprint` 降级为纯展示参考（「验证此终端」展开仍可见，供与对端屏幕上的指纹人工比对）。
- 信任管理：设置页「已信任终端」列表 + 移除信任（下次该终端提议重新走 TOFU 弹窗）。移除不影响进行中会话。
- **不做**：导出/编辑信任表、自动接收模式（无人值守）、跨终端同步——全部转 §7 TODO。

### 5.5 数据面协议（VLF/1）与断点续传

一条 TCP 连接 = 一个任务的生命周期实例（断线重连开新连接）。帧统一 `[u32 BE 帧长][密文载荷]`，**帧长先校验再分配**（上限 1 MiB，超限立即断连记日志）。文件正文按 ≤1 MiB 分帧流式传输（受上述帧长上限约束）。

```
连接（发起方 dial 接收方公告的 ip:tcpPort）
  ├─ 明文握手 ×2（双向，超时 10s 总）：
  │    magic "VLF1" | version u8=1 | role(initiator|responder)
  │    | peerId | ed25519 公钥(32B) | X25519 临时公钥(32B)
  │    | sessionNonce(16B, 发起方先生成、应答方拼接) | 发起方&应答方各自 ed25519 签名
  │    签名覆盖：magic+version+双方公钥+双方 sessionNonce+方向角色
  │    → 接收方校验：签名验证通过 && multihash(公钥)==peerId（协议固有一致性检查）
  │    → HKDF-SHA256(IKM=X25519(临时私钥,对方临时公钥),
  │         salt=nonceA‖nonceB‖peerIdA‖peerIdB, info="vitry-lan-file-v1")
  │       → c2s / s2c 两把独立 ChaCha20-Poly1305 密钥
  ├─ 加密 Initial 帧：
  │    发起方 → Offer { transferId, files:[{name,size}], totalBytes }
  │             或 Resume-Offer（同 transferId，携带 resumeHint=true）
  │    接收方 → Accept{fresh} | Accept{resume, perFileReceivedBytes[]} | Reject{code}
  ├─ 正文帧：Chunk { fileIndex, seq } AAD 绑定（会话 + fileIndex + seq），
  │    nonce = 8B 会话随机前缀 + 4B 逐方向计数器（2^32 帧前禁止复用，超限断连重建）
  ├─ End { perFileSha256[] } → 接收方全文件重哈希对账 → rename → Ack{ok|corrupt}
  └─ 任意时刻 Cancel{reason}（双向）
```

- **无帧级硬超时**（预研 kimi 教训）：仅 TCP 连接 5s、握手 10s、提议响应 60s（§5.4）、续传窗口 120s（下）。
- **续传规则**（核心语义，用户定案）：
  - `.tmp` + sidecar `AppData/lanfile/.<transferId>.meta.json`：`{transferId, direction, peerId, files:[{name,tmpName,receivedBytes}], cancelled?}`。
  - **仅异常中断**（TCP 断且本端未取消）→ sidecar 保留，进入 120s 窗口；发起方自动重连（指数退避 1/2/4/8/16/32s，封顶窗口），dial 用**新连接但同 transferId + resumeHint**。
  - 接收方见 resumeHint：查 sidecar **且无 `cancelled` 墓碑且窗口未过** → `Accept{resume}`，从各文件 `receivedBytes` 边界续发（发起方对已传文件 seek 跳过）。
  - **用户显式取消（任一侧）→ sidecar 写 `cancelled` 墓碑、删 `.tmp`，保留墓碑至窗口自然过期**：此后对端重连一律 `Reject{cancelled}`；**取消 = 不再续传，须用户重新发起新任务**。
  - 超窗无重连 → 删除 sidecar 与 `.tmp`，任务 `failed(transfer_failed)` + notify warning。
  - 接收方**进程重启**：启动时扫 `.tmp`/sidecar，超过上次 mtime + 120s 的直接清理；窗口内的保留等 dial（发起方重试撞上即可续）。
  - **resumed 会话不重发 `lan-file://incoming`**（不弹第二次面板）。
- **完整性**：逐帧 AEAD tag + 每文件结束时 SHA-256 全量重读对账（磁盘速度 ≫ 坏链路带宽，重读成本可忽略）；不匹配 → `integrity_mismatch`，删全部 `.tmp`，任务失败。
- **落盘安全**：展示/落盘文件名 `sanitize_filename`（去路径分隔符/控制字符/盘符，防目录穿越）；重名自动 `name (1).ext`；正式名经 `.tmp → rename` 原子出现；接收前检查目标盘剩余空间 ≥ `totalBytes + 200MB`，不足 `Reject{disk_full}`（发送侧收到后任务失败 + notify）。

### 5.6 发送侧输入与校验

- 前端经 `tauri-plugin-dialog` 原生选择器（多选）或拖放得到**真实路径列表**，`sendLanFile` 原样传后端。
- 后端逐条校验（dt 点）：存在且为**普通文件**（目录 → `invalid_path`，v1 不支持目录）；可读（打开探测 → `file_unreadable` / `file_not_found`）；单任务文件数 ≤ 100、路径 ≤ 500 字符（超限 `invalid_path`）。任一不满足 → 命令直接 Err，任务不创建。
- 传输过程中源文件被删/截断 → 读错误即任务 `failed(file_unreadable)`。

### 5.7 自动图片通道（与 lan-sync 联动）

lan_sync 的 capture 钩子在**新图片条目**产生时（与剪贴板广播同一触发点）追加：

1. `imageMeta` 信封增字段（契约 lan-sync 5.6 同步更新）：`hash`（图片字节 SHA-256 hex）、`xfer: true`；信封 `v` → `"0.3.0"`。
2. **桌面接收端**免确认落盘条件（全部满足，否则维持占位、静默）：
   - 来源 peerId ∈ 本端信任表（Q9 定案：不接受未信任节点的自动字节；peerId 即身份，信任表按 peerId 键控）；
   - `caps` 含 `img` 且 ImageOffer 的 `hash` 与信封 `imageMeta.hash` 对得上（关联键）；
   - 字节 ≤ **10 MiB**；类型判定 = Offer variant `kind:"image"` + 扩展名白名单 png/jpg/jpeg/gif/webp/bmp；
   - 磁盘检查同上。
3. 落盘 `AppData/lan-inbox-images/<hash>.<ext>`（**不**进 `AppData/lanfile`）；LRU 上限 **200 张 / 500 MB**，超限删最旧（按文件 mtime；正在被收件箱预览引用不加锁，v1 接受）。
4. 收件箱条目点亮：按 `imageMeta.hash` 查该目录，存在 → 真实缩略图/预览；不存在 → 占位（现状行为）。**任何失败静默**（无事件、无错误 UI，仅日志）。
5. 开关矩阵：`setLanFileEnabled(false)` 或 `setLanSyncReceive(false)`（接收侧）→ 不接收落盘；`setLanSyncBroadcast(false)` 或 `setLanFileEnabled(false)`（发送侧）→ 不发自动传输（信封元数据广播照常）。
6. 发起侧排队（复审修订 D①）：**先卡发送侧门槛——图片字节 > 10 MiB 直接不排队（不发注定被拒的传输）**；对每个「在线 + `caps(img)`」终端各入队一次 ImageOffer（同 5.5 数据面，Initial 帧 variant `{kind:"image", name, width, height, size, hash}`），串行小队列（5.3），互不影响。信任判定归**接收端**（信任表单边持有，发送端无从知晓对端是否信任自己；不信任即静默不落盘，占位照常）。

### 5.8 与现有功能的关系

- `lan-sync` 收件箱**不新增文件类条目**：交互传输的文件只落 `AppData/lanfile`，收件箱不记（无历史列表决策 Q7 的延伸）；收件箱仅图片点亮变化（5.7）。
- 回写/历史链路：**桌面**图片条目**已点亮**（字节已在 `lan-inbox-images/`）→ 点「复制到剪贴板」写回**图片字节**（经 clipboard-x 写图，与本地截图写回同路径）；**未点亮** → 维持占位文本 `[图片] 名称 (宽x高)` 写回（现状行为）。移动端仅纯文本，写回不受点亮影响。（写回语义变更经契约终审确认 2026-08-20；实现时同步更新契约 lan-sync 5.5 / mobile 5.2 相应句）
- notify 接入：任务 done/failed/cancelled/对端取消/超窗 → 后端 `notify_app`（level info/warning），错误码即 §4。

### 5.9 移动端差异（0.3.0）

- 移动端 = **仅图片通道接收端**：注册 peer_node 公告（`caps:["img"]` + 动态端口监听）与 ImageOffer 接收/落盘（5.7，含 §4 磁盘检查）；无「文件」页、无提议面板、无 7 命令（均不注册）、无设置区、无托盘。
- 移动端信任引导（替代人工 TOFU，复审修订 A 表述更新）：手机无确认 UI 载体，免人工落图需**两层证据**——① **身份真实性**：libp2p 连接层 noise 已认证 peerId + VLF 握手校验 `multihash(公钥)==peerId`（协议固有检查，确保 TCP 连接方与公告方是同一身份）；② **新鲜观察**：该 peerId 在**剪贴板主题上 5 分钟内出现过**（手机自己看到它活跃广播过剪贴板消息，排除「刚接入网段的陌生机器推图」）。满足 → 免人工确认落图（≤10 MiB，其余规则同桌面 5.7-2/3）。理由：纯「同网段任何人可写图」被否（Q9），② 是主要关口。（终审已确认 2026-08-20）
- 生命周期 = 应用进程（前台；退后台监听 socket 随进程冻结/回收即失效，重进前台重听；无后台保活，沿 0.2.9 限制）。
- 桌面 `sendLanFile` 到移动端 peerId → `lan_file.peer_unsupported`（`caps` 无 `file`）。

## 6. 破坏性影响

- 全新功能，不动既有命令语义。**增量改动清单**：
  1. `core::peer_node`：`Publish`/`PubsubMessage` 增加 `topic` 字段（内部 API，非前端契约面）。
  2. `lan-sync` 信封 `v` → `"0.3.0"`，`imageMeta` 增 `hash`/`xfer` 可选字段（旧版忽略，向后兼容；契约 lan-sync 5.6 同步）。
  3. `set_tray_labels` 新增**可选**参数 `fileShare: Option<String>`（**None = 保留「文件共享」菜单项现文案**，旧调用不破；契约 quick-paste 5.5 同步）。
  4. **桌面**收件箱写回语义微调：已点亮图片条目写回图片字节而非占位文本；移动端维持纯文本/占位不变（契约 lan-sync 5.5 / mobile 5.2 同步）。
  5. `Cargo.toml` 新增：`x25519-dalek`、`chacha20poly1305`、`hkdf`、`sha2`、`rand`、`ed25519-dalek`（身份签名复用 peer_key）；`tokio` 增 `net`/`io-util` feature；桌面依赖组新增 `tauri-plugin-dialog`（+ capabilities 权限）。
  6. 新文件：`AppData/lan-file.json`、`AppData/lanfile/`、`AppData/lanfile/.<id>.meta.json`、`AppData/lan-inbox-images/`。
- 版本 0.3.0 三处同步 + CHANGELOG + `docs/features/lan-file.md` + README（新功能 + 已知限制：防火墙、无目录/无人值守接收、Windows 虚拟网卡组播限制沿用）。

## 7. 未决问题（转 TODO，不进 0.3.0）

- 共享文件夹/目录树浏览（B 模式）、文件夹拖入（保留相对结构）、移动端双向（C，用户已排期 TODO）。
- 无人值守自动接收（需磁盘配额与清理策略）、信任列表导入/导出、跨端信任同步。
- 多任务队列（并发 N）、传输历史持久化（本会话摘要替代）、暂停/恢复显式语义。
- libp2p-mdns Windows 多网卡组播出口根治（fork 提 PR 或移植手写 mDNS 的 `IP_MULTICAST_IF` 方案，预研 §7.2；当前公告层因走已建立的 libp2p 连接而受牵连较小——公告失败即互相发现不了，与 lan-sync 同坑同修）。
- gossipsub 公告的 MAC 泛洪观察（100 节点级局域网每 5min 一条，量级安全；极端规模再议）。
- 图片通道失败时的可观测性（当前完全静默；考虑设置页计数诊断）。
