# 局域网文件共享（lan-file）

- 状态：开发中（契约终审通过 2026-08-20，实现进行中）
- 接口契约：[docs/api/lan-file.md](../api/lan-file.md)
- 后端 mod：`src-tauri/src/features/lan_file/`
- 前端目录：`src/features/lan-file/`

> 版本 0.3.0｜调研与决策：`lan_file_test/五模型局域网文件传输预研评测报告.md`（五方案横评）+ /grill 会话（后端 Q1–Q12、前端 F1–F7）

## 目标

在 lan-sync（剪贴板同步）之外补齐「大内容」通道：局域网内桌面终端之间**点对点推送任意文件**——不需要 U 盘、聊天工具或云盘，选中文件点一下对方终端即可；同时把剪贴板同步遗留的「复制截图、他端只见占位」补全为**真实图片自动点亮**。所有数据只在局域网内流转，传输全程加密、落盘前完整性对账。

## 使用场景

- **传一个巨大的文件**：显示器上录了个 2GB 演示视频，直接推给同事的 VitryTool——对方屏幕顶部滑出提议面板（文件名 + 大小 + 60s 倒计时环），点「接受」即开传，传输卡显示实时速率与子文件进度；Wi-Fi 抖一下断线，界面只浮现「网络中断，等待恢复…」琥珀横幅，链路回来自动从断点续传，全程无需操作。
- **截图秒到对端**：和一台已互相信任的终端之间，随手 `Win+Shift+S` 截屏 → 对方收件箱里那条 `[图片]` 占位交叉淡入成真实缩略图，点开浮层看大图——字节走后台自动小通道，不打扰、不弹窗。
- **首次相遇**：陌生终端第一次发文件，对方需要点一次「接受」（TOFU）；此后该终端的文件提议直接进入传输，不再询问。对方若重置了身份顶旧名回来，收到的是红色警告卡。

## 架构位置

```
features/clipboard_history ──(新图片条目 is_new)──▶ core/hooks ──▶ features/lan_file（ImageOffer 排队，§5.7）
                                                                    │
「文件」页 / 提议面板 / 传输卡（前端）──invoke / 事件──▶ features/lan_file 命令薄壳
                                                                    │
                                    service.rs（会话状态机 / TOFU 信任表 / 公告 peers / 落盘与 sidecar）
                                                                    │
                              transport/{proto.rs, crypto.rs}（VLF/1 帧编解码 + 握手/HKDF/AEAD，纯逻辑可测）
                                                                    │
                             tokio TCP 监听(0.0.0.0:0) ＋ core/peer_node（mDNS 发现 + gossipsub 公告，跨功能复用）
```

- **core/peer_node（增量）**：`Publish` / `PubsubMessage` 增加 `topic` 字段（多主题通道，业务语义仍留在各 feature）；公告主题 `vitrytool-lan-file-announce`（`{v, peerId, terminal, tcpPort, fingerprint, caps}`，`caps: file/img`）。
- **features/lan_file**：`commands.rs`（7 命令薄壳）/ `service.rs`（状态机 + 信任 + 公告 peers）/ `state.rs`（共享态 + 单会话槽）/ `store.rs`（`lan-file.json`：开关 + 信任表；`AppData/lanfile/` 落盘 + sidecar `.meta.json` 续传墓碑）/ `transport/`（协议编解码与密码学原语组装，不碰磁盘与 UI）。
- **数据面独立裸 TCP**（预研五家共识）：文件正文不经 libp2p 流（gossipsub 64KiB 上限 / yamux RTT 锁吞吐 / Windows dial 挂起三大实测缺陷，预研 §1.2、§4.5）；libp2p 只做发现与端口公告。
- **前端**：新「文件」导航页（拖放区 + 终端卡网格 + 传输卡，移动端隐藏）；提议面板（顶部下滑 + 60s 环，主窗隐藏时唤窗）；托盘「文件共享」快速开关（hooks 模式）；收件箱图片点亮 + 浮层预览。
- **单实例 + 持久身份**（lan-sync 0.2.5 已建）：一台机器一个节点，peerId = ed25519 公钥 multihash——**身份即指纹**，无需另设指纹信任机制。

## 数据流（关键路径）

```
发送：pickFiles(dialog)/拖放 → sendLanFile(peerId, paths) →（后端校验存在/普通文件/≤100）
  → dial 对端公告 ip:port → VLF/1 握手（双向签名 + X25519 → HKDF 双向密钥）
  → 加密 Offer{transferId, files} → 对端 lan-file://incoming →（用户 accept / 60s 超时）
  → 逐文件 ≤1MiB 分帧流式（Chunk，AAD 绑定 fileIndex+seq）→ 每文件 End{sha256} → 对端重读对账 → Ack
  → 全部完成 → done（双方 notify；接收侧原子 rename 出正式名）

断线：TCP 断（本端未取消）→ 接收侧 sidecar 保留进 120s 窗口，发端面板态转 resuming
  → 发起方指数退避重连（同 transferId + resumeHint）→ 接收方查 sidecar 无墓碑 → Accept{resume, offsets} → 续发
  （窗口内 resumed 不再弹提议面板；超窗 → 双侧 failed + 清理）

取消：任一侧 cancel → Cancel 帧尽力送达 + 本地 sidecar 写 cancelled 墓碑、删 .tmp、清内存
  → 对端收到 Cancel 或重连撞墓碑 → 任务终止，**永不自动续传**（须重新发起）

图片通道：capture 新图片 → imageMeta 信封加 hash/xfer + 对每个在线 caps(img) 终端入队 ImageOffer
  →（接收端门控：桌面=信任表含 peerId / 移动=noise 认证+5min 剪贴板新鲜观察，≤10MiB，白名单扩展名）
  → 免确认落盘 AppData/lan-inbox-images/<hash>.<ext>（LRU 200 张/500MB）→ 收件箱条目按 hash 点亮
  （任何一步不满足 → 静默不落盘，收件箱维持占位，无错误 UI）
```

## 安全与边界

- **加密**：每次传输临时 X25519 → HKDF-SHA256 派生 c2s/s2c 双向独立密钥 → ChaCha20-Poly1305 分帧 AEAD（nonce = 8B 随机前缀 + 4B 方向计数）；被动嗅探拿不到文件名与内容（预研 glm-5.3 嗅探实证同构方案）。
- **身份**：握手双向 ed25519 签名（长期身份密钥签本次临时密钥与会话 nonce）——TCP 连接方与公告 peerId 密码学绑定，MITM 无法在不被察觉下顶替已信任终端。
- **人工关口**：交互传输必须接收方确认（首次 TOFU，含 60s 时限自动拒绝）；自动图片通道的免确认**只**对已信任桌面节点 / 「noise 认证 + 剪贴板新鲜观察」移动节点开放，且 ≤10MiB + 图片扩展名白名单。
- **落盘防线**：文件名净化（防目录穿越）+ 重名自动改名 + `.tmp` 原子 rename + 磁盘空间预检（总大小 + 200MB 余量）+ 全文件 SHA-256 对账失败即丢弃（预研安全底线第 5 条）。
- **DoS 卫生**：帧长先校验再分配（上限 1MiB）；公告 12min 过期驱逐；图片小队列深度 8 丢旧；单任务 ≤100 文件。
- **鲁棒性**：数据面无帧级硬超时（预研 kimi 30s 超时中断 128MB 的教训——坏链路可扛 >30s 停顿）；断线续传仅对「异常中断」开放，用户取消 = 永久终止（人工决定不被自动化越过）。
- **边界（v1 不做）**：目录树/共享文件夹、无人值守接收、多任务并发、传输历史持久化、暂停/恢复显式语义、移动端发起——见契约 §7。

## 测试要点（dt）

- `transport/proto.rs`：帧编解码往返、超长帧拒绝、握手帧序列字节级往返、AAD 篡改拒绝、nonce 不重复。
- `transport/crypto.rs`：HKDF 双向密钥派生确定性/独立性、ChaCha20-Poly1305 往返、错误密钥失败、签名验证通过/拒绝（含 multihash(公钥)==peerId 一致性）。
- `service.rs` 纯逻辑：`sanitize_filename`（路径穿越/控制字符/盘符/空名 ≥7 组）、重名改名、状态机全转移（offer→accept→transfer→done / reject / timeout / cancel / resume / 超窗，注入假时钟）、续传 offset 计算、取消墓碑拦截重连、TOFU 信任表读写、nameClash 判定、公告过期驱逐、图片通道门控矩阵（信任×大小×扩展名×开关）、磁盘检查纯函数、`bytesPerSec` EMA。
- 集成：tokio 回环双实例端到端（小文件全链路 / 篡改 mid-stream 检测 / 断连重连续传 / 取消墓碑），`#[tokio::test]`。
- 前端 vitest：api 封装、FilePage（拖入预填/disabled/即发起）、OfferPanel（倒计时/形变/TOFU 展开/nameClash 警告卡）、TransferCard（七态渲染/resuming 横幅/reduced-motion）、收件箱点亮与预览、Settings 状态行与移除信任。
- 真机验收（硬性，用户指定）：Windows ↔ silverbox（`sovlyn@192.168.31.203`）双机矩阵——双向发现、互发（文本/大文件/多文件）、拔线断点续传、取消不续、TOFU 首确、截图自动点亮、磁盘满负向。**通过才签发 0.3.0。**

## 已知限制（README 同步）

- **Windows 防火墙可能拦截首次入站 TCP**：新程序在 Public 档案下默认被拦 → 首次使用需允许 VitryTool 通过防火墙（弹窗或手动添加入站规则）；未放行表现为「对方终端发不过来」（dial 超时），本机仍可主动外发。
- **Windows 虚拟网卡 mDNS 发现坑沿用 lan-sync**（见 lan-sync 文档）：WSL/Hyper-V 虚拟网卡可能使终端互相发现不了 → 公告随之不可达；规避方式同 lan-sync。
- 单会话：同一时刻只能进行一个传输任务；忙时新提议被自动拒绝。
- 断点续传只认「异常中断」：任何用户取消/拒绝/超时后的断链都不保留进度。
- 无传输历史：完成后仅本会话内折叠摘要，重启应用即无痕（落盘文件本身保留）。
- 移动端仅接收图片通道产物，且应用需前台；桌面→手机传文件、手机→任何端发起均不支持。
- 文件类剪贴板广播（filePaths 元数据）维持占位文本现状，不自动触发传输（防「复制即外发」惊喜，TODO 讨论显式化入口）。
