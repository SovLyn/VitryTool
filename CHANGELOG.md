# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 与语义化版本约定（见 `docs/versioning.md`）。

## [0.3.0] - 2026-09-08

### 新增

- **局域网文件共享（lan-file，0.3.0）**：局域网内桌面终端之间点对点推送任意文件——
  仅确认后传输（TOFU 首确 + 60s 提议窗口 + 撞名红色警告卡）、多文件串行、
  断线 120s 宽限自动续传（用户显式取消 = 永久终止，sidecar 取消墓碑）、
  VLF/1 独立裸 TCP 数据面（X25519 → HKDF-SHA256 双向密钥 → ChaCha20-Poly1305
  分帧 AEAD，ed25519 握手签名与 `multihash(公钥)==peerId` 一致性校验，
  帧长先校验再分配上限 1MiB，落盘前磁盘预检 +200MB 余量 + 全文件 SHA-256 对账 +
  `.tmp` 原子 rename + 文件名净化防目录穿越）。
  - 后端：`features/lan_file`（8 命令 / 4 事件 / 状态机 / TOFU 信任表 / sidecar /
    公告 peers TTL 驱逐 / 图片通道 / 路径预检）；`transport/{crypto,proto,session}`；
    全仓 dt **149 项**（含 tokio 回环端到端、AAD 篡改拒绝、身份假冒拒绝、取消墓碑、
    续传偏移、取消安全帧读、对端取消帧识别、公告新旧判定）。
  - **自动图片通道**（与 lan-sync 联动，契约 5.7）：已信任终端复制截图 → ≤10MiB
    免确认落 `AppData/lan-inbox-images/`（LRU 200 张 / 500MB）→ 收件箱按
    `imageMeta.hash` 点亮真实图片，写回图片字节；任何失败静默降级占位。
  - core/peer_node 多主题化（`Publish { topic }` / `PubsubMessage { topic }`，
    lan-sync 行为不变）；gossipsub 公告主题 `vitrytool-lan-file-announce`
    （caps: file/img，公告自校验 `multihash(公钥)==peerId`，12min TTL 驱逐）。
  - 前端：「文件」导航页（拖放区 + 选择文件 + 终端卡网格 + 传输卡七态渲染 +
    resuming 琥珀横幅）、提议面板（顶部下滑玻璃 + 60s 倒计时环 + TOFU 指纹展开 +
    nameClash 红色警告卡位权反转）、设置页 lan-file 区（总开关 + 状态行 +
    已信任终端管理）、托盘「文件共享」快速开关（`setTrayLabels` 增可选 `fileShare`）；
    i18n 双语（`lanFile.*` 40+ 键）；vitest 新增 lan-file api / 组件用例。
  - 移动端（契约 5.9）：仅自动图片接收端，7 命令均不注册（`build_invoke_handler`
    平台拆分保证）。
- 已知限制（README 同步）：Windows 防火墙可能拦截首次入站 TCP（首启需放行）；
  Windows 虚拟网卡 mDNS 多网卡坑沿用 lan-sync；单会话无传输历史持久化；
  **不支持文件夹**（拖入即拦下并提示）；**链路差时靠 30s 停滞检测自动重连续传**
  （WiFi 丢包会让 TCP 窗口塌缩，吞吐可掉到几十 KB/s）。

### 变更

- lan-sync 信封 `v` → `"0.3.0"`，`imageMeta` 增可选 `hash` / `xfer` 字段
  （旧版忽略，向后兼容）；已点亮图片条目写回 = 图片字节（未点亮维持占位文本）。

### 修复

- **lan-file 真机联调（Windows ↔ Linux 双机实测）**：
  - **公告回声风暴**：收到对端公告后无条件回发自身公告 → 两端互相触发（gossipsub 消息 id 含 seqno，去重失效）→ 每端数十条/秒、日志 4 分钟涨到 4MB。现仅对「首次发现的对端」回发，回发路径加 3 秒最小间隔。
  - **发现被节流吞掉**：上述节流若作用于全部公告路径，会吞掉启动公告与「连接建立后补发」→ 双方互不可见直到 5 分钟周期公告。现节流只作用于回发路径。
  - **帧读取取消不安全**（致命）：等待应答用 200ms 分片轮询，`read_exact` 被取消会吞掉半帧 → AEAD 帧流失步（`frame too large: 2626586369`）→ 传输随机失败。改为「接收缓冲 + 可取消 `read()`」的读取器。
  - **对端取消被误判为断线**：`Cancel` 帧在独立 AAD 域，接收方按 Chunk 域解密失败后直接当断线处理 → 不写取消墓碑、`.tmp` 残留、状态显示 resuming。现按 cancel 域回退解密识别，正确写墓碑 + 删 `.tmp` + 终态 cancelled。
  - **续传把整份文件重发**（致命）：接收方进度向量未以 sidecar 偏移为初值 → 续传会话把 50MB 文件 append 成 55MB → 哈希对账失败 → 发送方误判可重试 → 无限循环。现两侧分块序号统一为 `offset / 1MiB`，接收方以偏移初始化。
  - **续传 `.tmp` 错位**：`.tmp` 连续写入而 sidecar 每 250ms 落一次，异常中断后 `.tmp` 常比记录偏移更长 → 直接 append 错位（文件 100% 传完却校验失败被丢弃）。现先截断到记录偏移；记录偏移 > 实际文件则清理判失败。
  - **重连沿用旧端口**：接收方重启换动态端口后，发送方仍 dial 首次解析的地址 → 永远重连失败。现每次（重）连都重新解析「IP + 当前公告端口」。
  - **语义失败不再重连**：对端拒绝 / 磁盘满 / 完整性不符 / 源文件不可读等直接终态（此前一律退避重连）。
  - **旧撤销公告误删已上线对端**：快速重启时旧实例的撤销公告可能晚于新实例公告到达。公告新增 `ts` 字段，比已记录公告更旧的公告（含撤销）一律忽略。
  - **大文件传输「卡几分钟再继续」**（真机实测 500MB：接收方 ~234MB 时长时间零进展，卡片冻结无提示）：
    发送侧日志只有一条 `断开的管道`（接收方取消时才报），TCP 统计显示 VLF 连接的
    `cwnd` 塌到 2 段、`ssthresh` 2–3、RTT 从 minrtt 66ms 涨到 174ms→1.2s、6 次重传 + DSACK 重复，
    而同一对机器上的 SSH 连接 RTT 仅 2.3ms —— WiFi 链路丢包 + 缓冲膨胀把窗口压塌，
    吞吐掉到 20–260 KB/s，观感就是「卡住」。代码侧此前**没有任何停滞检测**，只能干等。
    现：单块写入 30s 零进展即判链路已死 → 按断线处理（重连 + 断点续传），并新增定位日志
    （接收侧单块等待 >5s / 磁盘写 >1s、发送侧写块 >2s / 磁盘读 >1s）。
  - **卡顿恢复时的「续传被拒 busy」**：发送侧 30s 停滞重连时，接收侧旧会话可能仍占着会话槽
    （数据面无帧级硬超时，它会一直阻塞在读里）→ 合法续传被拒成 `busy` 而直接失败。
    现：① 会话槽引入 **epoch**，续传请求可**顶掉同一 transferId 的陈旧会话**（旧任务回来释放时
    校验 epoch，不会误清新会话）；② 接收侧新增 **60s 无数据看门狗** → 判链路已死、释放槽并保留
    sidecar 等重连；③ 发送侧遇 `busy` 且本次是续传时按可重试处理（退避后再来）。
  - **「文件」页一键打开接收文件夹**（用户要求）：按钮打开本机 `AppData/lanfile`（交互传输
    落盘目录）；`capabilities/default.json` 增带 scope 的 `opener:allow-open-path`
    （只放行该目录）。收到的文件不在系统「下载」文件夹，故不提供下载文件夹按钮。
  - **拖入文件夹点发送完全静默**（用户报告）：路径校验此前在**任务线程**里做，命令已返回
    `transferId`，前端既没有卡片也没有提示。现校验前移到命令层（直接 Err → 明确提示），
    并新增 `checkLanFilePaths` 路径预检：目录/不可读文件在**入列时**就被忽略并提示
    「暂不支持文件夹传输」，通过的文件顺带显示大小（契约 5.6 增量）。
  - **失败卡片改为「加回待传列表」**（用户要求）：失败/取消/被拒的发送任务不再提供「重试」
    （重试会替用户决定重发给同一终端，而失败往往正是链路/对端问题），改为把该次文件
    **加回已选列表**，由用户自行换终端或等链路恢复再发；接收侧失败无本机路径，不显示该按钮。
  - **「取消传输」点了没反应**（真机实测）：命令确实到达后端（日志有 `cancelled by user`），但三种状态下都无事发生——
    ① 接收侧任务已随连接结束退出、只剩续传窗口内的 sidecar（卡片显示「等待恢复」）→ `cancel_transfer` 找不到活跃任务即返回；
    ② 接收侧在对端静默时阻塞在 `recv_chunk`（数据面无帧级硬超时）→ 只在块间轮询取消，取消要等下一块；
    ③ 发送侧在 dial 退避睡眠中（最长 32s）→ 期间不轮询取消。
    现：无活跃任务但有 sidecar → 直接写墓碑、删 `.tmp`、删 sidecar 并推 cancelled 终态；接收侧分片等待中轮询取消（≤200ms 生效）；
    发送侧退避改为分片睡眠并轮询取消；无任务无 sidecar 的残留卡片也推空闲收束 UI。
  - **收件箱图片点不亮（asset 协议拒绝路径）**：前端用**相对路径** `lan-inbox-images/<hash>.<ext>` 调
    `convertFileSrc`，后端报 `asset protocol not configured to allow the path`（scope 只认绝对路径）→ 永远维持占位。
    现用 `appDataDir()` + `join()` 拼绝对路径（新增 vitest 用例锁住该行为）。
  - **设置页「已信任终端」空态提示与列表同时显示**：状态行显示「已信任 1」时，下面仍显示
    「尚无已信任终端。首次接收文件提议时确认即建立信任。」——空态提示此前**无条件渲染**，
    未包在「列表为空」判断里。现仅在没有已信任终端时显示（新增 2 个 vitest 用例）。
  - **真机联调批次（早前）**：公告 5 分钟发现盲区（连接建立后立即补发）、`listening=false` 导致双方零公告互不可见（端口绑定后立即回传）、`tauri-plugin-dialog` 漏注册、`ip_from_multiaddr` 提前返回空串（双端互发零反应）、`PeerConnected` 地址改取连接端点。
- **提议接受/拒绝链路**：接收任务发出 `lan-file://incoming` 前未登记待决提议，
  `acceptLanFile` 查表落空恒报 `lan_file.peer_not_found`（面板点「接受」立即失败）——
  现注册待决表并在决定送达/超时/任务结束时移除；**已信任终端免确认不再弹面板**。
- **终态停留展示**：任务进入 done/failed/cancelled/rejected 后不再紧跟空闲快照抹掉卡片；
  发送侧补 peerId/终端名与终态快照；发出 Initial 帧、收到 Accept、以及**退避重连期间**各推一次
  offering/transferring/resuming 快照（此前 dial 重试 63s 内前端毫无反馈）。
- **续传窗口完整化**：接收侧连接中断转 `resuming` 快照并启动 120s 超窗检查
  （超窗清 sidecar 与 `.tmp` + `failed(transfer_failed)` + 通知）；启动时清理超窗残留工件；
  同一时刻第二个新提议自动拒绝时本端补 warning 通知。
- **自动图片通道打通**：`imageMeta.hash`/`xfer` 改为**广播前**回填（此前从未写入 →
  收件箱条目没有关联键、字节到了也点不亮）；收件箱点亮探测缓存在 `image-arrived`
  事件后失效重探测（此前首次 miss 永久缓存）。
- **移动端免人工信任门控**：信任表恒空的移动端此前一律拒收图片字节，现按契约 5.9
  以「VLF 握手已认证 peerId + 5 分钟剪贴板主题新鲜观察」放行（桌面仍只认信任表）。
- **移动端（Android）编译修复 + 契约 5.9 落地**：`features::lan_file` 此前被
  `#[cfg(desktop)]` 整体排除，而 lan-sync 侧无条件引用它 → **Android 目标编译失败**
  （10 处 `cannot find lan_file in features`）。现两平台均编译并初始化：桌面 = 交互
  传输 + 提议面板 + 图片通道；移动端 = 仅图片通道接收端（公告 `caps:["img"]` +
  动态端口监听 + ImageOffer 接收/落盘），7 个交互命令仍不注册。
- **撤销公告**：关闭开关/退出时尽力广播 `caps: []` 撤销公告，接收侧收到即从列表移除
  （此前只能等 12 分钟 TTL 过期）。
- **前端**：传输卡新增「取消传输 / 重试（仅发送侧**未成功**的终态）/ 打开所在位置 / 关闭」；
  发起后**保留已选文件列表**并给出「已发起，等待对方确认」反馈；任意页面拖入文件即跳
  「文件」页并预填；新提议到达时唤出隐藏的主窗口；传输快照改为 **App 级监听**（切页/重挂
  不丢状态，此前切走页面期间的事件全部丢失）；本地取消不再显示「对方取消了传输」；
  传输卡错误码改为映射表 + `notify.unknown` 兜底。

## [Unreleased]

> 小改动批次（用户约定：不递增版本号、不打 tag）。

### 优化

- **剪贴板列表加载与 UI 性能**（接口契约不变）：
  - **复制零全量刷新**：`clipboard-history://updated` 事件捕捉路径载荷携带完整新条目
    （`listener.ts`）→ 主窗口历史页本地增量应用（新增 `src/features/clipboard-history/incremental.ts`
    纯函数：按 id 插入/置顶 + 按缓存上限镜像淘汰（收藏豁免）+ 展示序排序，与后端一致），
    消除大列表（接近 1024 条富文本，clipboard.json 最大 3.2MB 整体序列化 90-300ms）
    在每次复制时的后端往返与整棵 DOM 重建；全量刷新仅保留在低频路径（初次挂载、
    收藏切换、删除/清空、事件兜底）。契约 `docs/api/clipboard-history.md` 5.1 补事件载荷说明。
  - **大列表渲染**：`.entry-card` 启用 `content-visibility: auto` + `contain-intrinsic-size`
    （视口外卡片跳过渲染/布局，玻璃模糊成本随可见数比例化）+ 缩略图 `loading="lazy"`。
  - 前端新增 `incremental.test.ts`（8 用例）；vitest / tsc / build 全绿。
- **启动开屏动画**（防白屏优化批「加载占位」落地，升级为品牌 moment）：
  - `index.html` 内联零依赖开屏：galaxy logo 材质化浮现（scale + opacity + blur，临界阻尼
    cubic-bezier(0.22,1,0.36,1)）+「VitryTool」字标（半透明，用户确认）；logo 与 favicon
    同源 `/src/assets/logo.svg`（后续换 logo 只改一个文件，开屏自动跟随）。
  - 退场策略（`src/index.tsx`）：App 首次渲染完成与最短展示 500ms 取晚者 → opacity 淡出
    400ms（reduced-motion 150ms 纯透明度）→ 移除节点；生产启动快不闪一下、dev 冷启动
    期间持续覆盖防白屏。
  - 亮/暗：默认亮色、`prefers-color-scheme` 跟随系统、theme.tsx 应用 `data-theme` 后以
    已保存主题优先；reduced-motion 降级。
  - 顺手修正 `index.html` 脚手架默认 `<title>` 为「VitryTool」。

### 修复

- 无。

## [0.2.9] - 2026-08-17

### 新增

- **移动端（Android）支持**，契约见 `docs/api/mobile.md`、功能文档 `docs/features/mobile.md`：
  - **定位**：手机作为「接收 + 转发终端」——前台运行 libp2p 节点接收局域网剪贴板广播 → 收件箱 → 点条目写手机剪贴板 → 手动粘贴。**不监听**（Android 无可靠后台剪贴板监听）、**不广播**、**无后台保活**（首版）。
  - **平台隔离（编译期）**：`Cargo.toml` target 条件依赖（桌面 clipboard-x / global-shortcut / window-state / single-instance，移动 clipboard-manager）；`lib.rs` 按 `#[cfg(desktop)]` / `#[cfg(mobile)]` 门控插件注册、托盘、quick_paste、窗口事件钩子与命令列表（quick_paste / 托盘 / capture / cleanup 移动端不注册）；capabilities 拆桌面 `default.json` 与移动 `mobile.json`（按 `platforms` 字段生效）。
  - **`core/platform.rs`**：新命令 `getPlatformInfo`（`isMobile` / `platform` / `hotkeyCapability`，前端功能隔离唯一依据）；系统剪贴板写入平台分发（`write_text_plain` / 移动同步 `write_text_plain_sync`）；移动端可写文本提取（`mobile_writable_text`：text 优先 → html 剥标签 `strip_html` → 图片元数据占位）；全局快捷键能力判定从 quick_paste 迁入（core 自包含，`getHotkeyCapability` 命令改调，行为不变）。
  - **移动端回写**：`writeClipboardEntry` / `writeLanInboxEntry` 移动端写**纯文本**后**显式入历史/置顶**（复用 capture 落盘逻辑：指纹去重置顶/淘汰，不依赖 Android 剪贴板读权限）；lan-sync 经 `core::hooks::mobile_clipboard_write` 通道解耦调用 clipboard_history 实现；**不触发广播**。新错误码 `clipboard.write_unsupported`（files-only 条目兜底，前端禁用 + 提示）。
  - **Android 侧**：`gen/android` 脚手架（`tauri android init`）+ MainActivity 持有 `WifiManager.MulticastLock`（mDNS 组播接收必需）+ Manifest 权限（INTERNET / ACCESS_NETWORK_STATE / ACCESS_WIFI_STATE / CHANGE_WIFI_MULTICAST_STATE）。
  - **前端**：`src/api/platform.ts`（getPlatformInfo + 惰性缓存）；App 启动平台识别（移动端不启动剪贴板监听、不下发托盘文案）；设置页隐藏广播开关与快速粘贴组；收件箱 files-only 条目移动端禁用写回（`lanSync.writeUnsupported` 提示）；**响应式布局**（640px 断点：侧栏 → 底部磨砂 tab bar + 导航图标、安全区 `env(safe-area-inset-*)`、触控目标 ≥44px、toast 顶部居中）。
  - 新文案：`lanSync.writeUnsupported`、`clipboard.write_unsupported`，zh-CN / en-US 双语同步。
  - 后端 dt（strip_html / mobile_writable_text / 能力判定迁移等），前端 vitest 适配（平台识别异步）；cargo test / fmt / clippy、vitest 94、tsc、pnpm build 全绿。
  - **Android 构建验证通过**（本机，2026-08-17）：aarch64-linux-android 交叉编译（libp2p / clipboard-manager / tauri 全部编译通过）+ gradle 打包产出 `app-universal-debug.apk`（arm64-v8a）。
  - **真机 E2E 通过**（用户手机，2026-08-17）：无线调试安装 → 主界面/底部 tab/设置页隔离 → mDNS 发现（在线终端 3，MulticastLock 生效）→ 桌面复制 → 手机收件箱接收 → **点条目 → 其他应用粘贴出正确内容** → 显式入历史（历史页条目 + 收藏）。
  - **Android 启动图标品牌化**：新增 `scripts/render-android-icons.mjs`（galaxy.svg → 5 密度 mipmap 位图），替换 `gen/android` 默认图标（用户确认桌面图标生效）。
  - **CD**：`release.yml` 新增 Android job（setup-java + Android SDK + Rust Android targets + NDK + keystore secrets 签名 → `tauri android build --apk` → 上传 Release 草稿）；`gen/android/app/build.gradle.kts` 配置 release 签名（读 `keystore.properties`，存在才启用）。

### 修复

- **Android 黑屏（真机发现）**：`tauri.conf.json` 的 `windows` 数组在 Android 上会被全部创建——quick-paste 透明 popup 覆盖主界面。修复：popup 移入 `quick_paste::init` 代码创建（`WebviewWindowBuilder`，功能域仅桌面编译），`tauri.conf.json` 仅保留主窗口。
- **移动端设置页误报（真机发现）**：`Settings` 无条件调用 `getHotkey`/`getHotkeyCapability`（移动端命令未注册 → "Failed to save shortcut settings" toast）+ 「快速粘贴」组空标题。修复：平台识别后桌面才加载快捷键设置；快速粘贴整组（含标题）移动端隐藏。

### 变更

- 版本 0.2.8 → 0.2.9（三处同步）。
- `global_shortcut_supported` 判定逻辑从 `features/quick_paste/service.rs` 迁至 `core/platform.rs`（行为不变，core 自包含）。
- 环境（本机）：JDK 17（Microsoft OpenJDK）、Android SDK（cmdline-tools / platform-tools / build-tools / platforms 34-36）、NDK r26.1、Rust Android targets 安装完成（`dev/android-setup/download.mjs` 下载脚本，绕过 schannel TLS 故障）。

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
