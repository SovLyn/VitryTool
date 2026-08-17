# 架构与代码组织规范

本文档定义 VitryTool 的架构约定，是所有开发工作的依据。**接口契约先行**是首要原则。

## 1. 总体结构

```
VitryTool/
├── src/                  # 前端（SolidJS + TS + Vite）
│   ├── features/         # 按功能域划分的前端模块
│   ├── api/              # invoke 调用封装（前端唯一的后端通信入口）
│   ├── i18n/             # 国际化（资源 + 初始化）
│   └── App.tsx           # 应用入口组件
├── src-tauri/            # 后端（Rust）
│   └── src/
│       ├── main.rs       # 可执行入口（保持极薄）
│       ├── lib.rs        # 库入口：仅做 mod 声明与组装
│       ├── core/         # 横切基础：错误、状态、通用工具
│       └── features/     # 功能域，每个功能一个独立 mod
├── docs/                 # 公开文档
└── dev/                  # 内部启发式文档（gitignore 排除，不对外）
```

## 2. 前后端分离原则

- **物理隔离**：前端 `src/` 与后端 `src-tauri/` 互不引用对方源码。
- **唯一通信通道**：Tauri command（`invoke`）。前端**禁止**直接读取 Rust 状态、文件系统或 Tauri API 之外的宿主能力，一律通过 `invoke` 暴露的接口。
- **前端集中封装**：前端对后端的调用集中写在 `src/api/<feature>.ts`，组件内不散落 `invoke` 调用。
- **接口先行**：任何新接口必须先有契约文档（`docs/api/<feature>.md`），后实现。接口变更必须同步更新契约文档，禁止「只改代码不改文档」。
- **类型同步**：契约文档中同时给出 Rust（serde）与 TypeScript 的类型定义，实现时必须保持一致；后续可接入 tauri-specta 等工具做代码生成强制同步。

## 3. 后端组织（按功能分 mod）

```
src-tauri/src/
├── lib.rs            # mod 声明 + builder 组装 + invoke_handler 注册
├── core/
│   ├── error.rs      # 统一错误类型 ApiError（错误码 + 消息）
│   ├── state.rs      # 全局状态（AppState）
│   ├── log.rs        # 日志初始化（级别/目标/隐私约束，见 §8）
│   ├── hooks.rs      # 跨功能钩子（如剪贴板新条目 → lan-sync 广播，解耦功能间调用）
│   ├── tray.rs       # 系统托盘（应用壳能力）
│   ├── peer_node/    # 节点层（libp2p：身份持久化 + swarm 生命周期 + pubsub 通道，跨功能复用）
│   └── mod.rs
└── features/
    ├── mod.rs        # 各功能 mod 的声明与导出
    └── <feature>/    # 每个功能一个目录
        ├── mod.rs    # 公开命令与模块结构
        ├── commands.rs  # #[tauri::command] 定义（薄壳）
        ├── service.rs   # 业务逻辑
        └── tests.rs     # 开发者测试（dt）（必须）
```

规则：

- **一个功能 = 一个 mod**，自包含命令、业务与测试；`lib.rs` 只做组装，不写业务。
- 命令是**薄壳**：`#[tauri::command]` 函数只做参数解析与状态获取，业务逻辑放入 `service.rs`，以便脱离 Tauri 上下文独立测试。
- **开发者测试（dt）**：每个功能 mod 必须随功能编写开发者测试（单元测试或 doctest，覆盖度不做硬性要求）。
- 横切能力（错误、状态、日志）放 `core/`，功能间不互相依赖，需要复用走 `core/`。

## 4. 错误处理约定

- 后端统一返回 `Result<T, ApiError>`；`ApiError` 为结构化错误：稳定的**错误码**（如 `lan_share.peer_not_found`）+ 可读消息（兜底用）。
- **i18n 原则**：前端展示文案以错误码查本地化字典为准，后端消息仅作开发期兜底，不承担界面文案职责。
- 错误码表维护在功能契约文档与 `core/error.rs` 中。

## 5. 前端组织

```
src/
├── features/<feature>/    # 功能页面/组件（与后端 features/ 同名对齐）
├── api/<feature>.ts       # 该功能所有 invoke 调用封装（类型与契约文档一致）
├── components/            # 跨功能通用组件（如 StarIcon、NotificationProvider、ConfirmDialog）
├── i18n/
│   ├── index.tsx          # i18n 初始化与 Provider
│   └── locales/
│       ├── zh-CN.json
│       └── en-US.json
└── lib/                   # 通用前端工具（非功能）
```

## 6. 国际化（i18n）

- 前端基于 `@solid-primitives/i18n`，语言资源放 `src/i18n/locales/`。
- **开发阶段只支持 `zh-CN` 与 `en-US`** 两种语言；新文案必须同时写入两份语言文件。
- 后端不输出界面文案（见错误处理约定），避免双端重复翻译。

## 7. 测试策略

- 后端：`cargo test`（位于 `src-tauri/`）。**开发者测试（dt）**：单元测试放在功能 mod 的 `tests.rs`，文档示例使用 doctest；每个功能 mod 必须要有。
- 前端：vitest（`pnpm test`），配置见 `vite.config.ts`。
- CI（GitHub Actions：fmt + clippy + cargo test + 前端 build + vitest）规划中，见 TODO.md。

## 8. 日志约定

- 基于 `log` 门面 + `tauri-plugin-log`（官方后端），初始化在 `core/log.rs`；业务代码直接用 `log::trace! / debug! / info! / warn! / error!`，无需关心输出目标。
- 级别与目标：**开发（debug_assertions）** `Debug` 级输出终端，系统 crate 压到 `Info`——libp2p 各子 crate 的 Debug/Trace（swarm 轮询、心跳、流协商）与 tracing→log 桥的 span 记录统一丢弃（自定义 filter，见 `core/log.rs`）；**发布（release）** `Error` 级写入应用日志目录文件（5 MiB 大小轮转，保留最近一份）。
- **隐私约束**：日志只记录元数据（id / 类型 / 长度 / 路径 / 操作结果），绝不记录剪贴板明文等敏感内容（见 `core/log.rs`）。

## 9. 平台差异（桌面 / 移动端，0.2.9）

移动端（Android）支持采用**编译期平台隔离**，契约见 `docs/api/mobile.md`、功能文档 `docs/features/mobile.md`：

- **依赖隔离**：`Cargo.toml` 用 target 条件依赖——桌面段（`cfg(not(any(target_os = "android", target_os = "ios")))`）挂 clipboard-x / global-shortcut / window-state / single-instance，移动段挂官方 clipboard-manager。`desktop` / `mobile` cfg alias 由 tauri-build 注入（Cargo target 表不认该 alias，须写显式 `target_os`）。
- **注册隔离**：`lib.rs` 用 `#[cfg(desktop)]` / `#[cfg(mobile)]` 门控插件注册、托盘 init、quick_paste init、窗口事件钩子与**命令列表**（`generate_handler!` 为 proc macro，参数内不能写 cfg，按平台拆两套完整列表）。
- **功能域隔离**：桌面专属功能（quick_paste、core/tray）整个 mod `#[cfg(desktop)]` 不编译；移动端命令（capture、cleanup 等）不注册、前端无入口。
- **平台识别**：`core/platform.rs` 提供 `getPlatformInfo` 命令（`isMobile` / `platform` / `hotkeyCapability`）+ 剪贴板写分发（桌面 clipboard-x / 移动 clipboard-manager）+ 移动端可写文本提取（`strip_html` / `mobile_writable_text`）；全局快捷键能力判定也从 quick_paste 迁入（core 自包含）。
- **capabilities**：按 `platforms` 字段拆桌面 `default.json`（clipboard-x）与移动 `mobile.json`（clipboard-manager）。
- **前端**：启动时调 `getPlatformInfo`，以 `isMobile` 隔离桌面功能（剪贴板监听、托盘文案、快速粘贴/广播设置、files-only 写回禁用）；响应式布局（640px 断点底部 tab）经 CSS media query，与平台无关。
- **Android 工程**：`gen/android/` 为 Tauri 脚手架（随仓库提交，构建产物 `build/`、`.gradle/`、`keystore.*` 被其 `.gitignore` 排除）；`tauri.conf.json` 的 `windows` 数组**只声明桌面主窗口**——Android 上该数组会被全部创建（quick-paste popup 覆盖主界面，0.2.9 实测坑），桌面小窗改由 `quick_paste::init` 代码创建。
- **构建注意**：Windows 上 `tauri android build` 的 jniLibs symlink 步骤需开发者模式/管理员；本机可用「手动复制 .so 到 `jniLibs/<abi>/` + gradle 排除 rust 任务」绕过（NDK 交叉编译需 `CC_aarch64_linux_android` / `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` 环境变量）。
