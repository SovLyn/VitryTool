# 功能名：剪贴板历史（clipboard-history）

- 状态：`草案`
- 关联功能文档：[docs/features/clipboard-history.md](../features/clipboard-history.md)（签发时创建）
- 版本影响：`minor`（0.0.1-alpha → 0.1.0）

## 1. 概述

持续捕捉用户剪贴板变化，保存为可浏览、可回写的历史记录（文本 / HTML / RTF / 图片 / 文件引用，原始格式保真），受数量上限约束；图片由 tauri-plugin-clipboard-x 落盘于其默认目录，通过即时淘汰 + 定时兜底清理维持"记录 ↔ 文件"一致。

领域术语见 `dev/CONTEXT.md`。架构决策见 `docs/adr/0001-clipboard-capture-via-webview-events.md`。

## 2. 命令列表

| 命令 | 方向 | 说明 |
| --- | --- | --- |
| `captureClipboard` | 前端 → 后端 | 监听事件触发：读剪贴板、落盘图片、去重置顶、即时淘汰；无可用内容返回 `null` |
| `getClipboardHistory` | 前端 → 后端 | 读取全部条目（最新在前），计算图片缺失派生标记 |
| `writeClipboardEntry` | 前端 → 后端 | 回写：按条目原始格式写回系统剪贴板 |
| `deleteClipboardEntry` | 前端 → 后端 | 删除单条条目并删除其图片文件 |
| `clearClipboardHistory` | 前端 → 后端 | 清空全部条目并清空图片目录 |
| `cleanupOrphanImages` | 前端 → 后端 | 定时兜底：扫描图片目录，删除无条目引用的孤儿图片 |
| `getMaxEntries` | 前端 → 后端 | 读取当前上限 n |
| `setMaxEntries` | 前端 → 后端 | 设置上限 n（1~1024），超限立即截断 |

## 3. 类型定义

### 条目（前后端共享）

```ts
// TypeScript（前端视角）
interface ClipboardEntry {
  id: string;             // 唯一标识（UUID）
  capturedAt: string;     // ISO 8601，捕捉时刻（后端取系统时间）
  text?: string;          // 纯文本
  html?: string;          // 原始 HTML（保真）
  rtf?: string;           // 原始 RTF（保真）
  image?: {               // 图片（本体由插件落盘，此处仅引用）
    path: string;         // 插件保存的 .png 绝对路径
    size: number;         // 字节
    width: number;        // 像素
    height: number;       // 像素
    missing: boolean;     // 派生字段：文件是否已不存在（不持久化）
  };
  files?: {               // 文件引用（不复制本体）
    paths: string[];      // 源文件绝对路径
    size: number;         // 总字节
  };
}
```

```rust
// Rust（后端视角，serde 序列化/反序列化）
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardEntry {
    pub id: String,
    pub captured_at: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub rtf: Option<String>,
    pub image: Option<ClipboardImage>,
    pub files: Option<ClipboardFiles>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardImage {
    pub path: String,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub missing: bool, // 派生，写入 store 时置 false 或忽略
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardFiles {
    pub paths: Vec<String>,
    pub size: u64,
}
```

### 请求 / 响应

```ts
// 请求
interface SetMaxEntriesReq { maxEntries: number }   // 1~1024

// 响应
type CaptureClipboardResp = ClipboardEntry | null;  // null = 静默忽略（无可用内容）
type GetClipboardHistoryResp = ClipboardEntry[];    // 最新在前
interface CleanupOrphanImagesResp { removed: number }   // 删除的孤儿图片数
interface SetMaxEntriesResp { maxEntries: number; evicted: number }  // 生效值 + 因截断删除的条目数
type GetMaxEntriesResp = number;
```

## 4. 错误码

| 错误码 | 含义 | 中文文案建议 | 英文文案建议 |
| --- | --- | --- | --- |
| `clipboard.capture_failed` | 读取剪贴板失败（插件底层错误） | 读取剪贴板失败 | Failed to read clipboard |
| `clipboard.entry_not_found` | 目标条目不存在（回写/删除） | 条目不存在 | Entry not found |
| `clipboard.invalid_max_entries` | n 超出 1~1024 | 上限需在 1 到 1024 之间 | Max entries must be between 1 and 1024 |
| `clipboard.storage_error` | store 读写/序列化失败 | 历史数据读写失败 | Failed to read or write history data |

## 5. 行为说明

### 5.1 数据流总览

```
系统剪贴板变化
  → 插件 Rust 监听线程（startListening）
  → emit "plugin:clipboard-x://clipboard_changed"（仅到 WebView）
  → 前端 onClipboardChange 收到
  → invoke captureClipboard
  → 后端 service：读格式 → 落盘图片 → 去重置顶 → 即时淘汰 → 写回 store
```

监听由前端在应用挂载时发起：`startListening()` + 注册 `onClipboardChange`；窗口关闭即退出（进程终止，监听随之结束）。**后端无自有时钟，全部业务动作由前端发起**（ADR 0001）。

### 5.2 captureClipboard

1. 依次尝试读取各格式，**逐格式独立容错**（某项失败不影响其他项）：`hasText→readText`、`hasHtml→readHtml`、`hasRtf→readRtf`、`hasImage→readImage(不带 save_path，用插件默认目录)`、`hasFiles→readFiles`。
2. 全部格式均无 → 返回 `null`（静默忽略，不报错、不产生记录）。
3. 图片读取成功后，以插件返回的 `{path,size,width,height}` 填充 `image`。
4. **去重置顶**：以内容指纹查找既有条目——文本按 `text` 内容相等；图片按 `image.path` 相等；无文本的纯富文本按 `html`/`rtf` 相等。命中则更新其 `capturedAt` 并置顶（图片路径相同、文件已存在，插件不会重复落盘）；未命中则插入新条目（`id` 为 UUID）。
5. **即时淘汰**：若条目数 > n，移除最旧条目；该条目若有 `image.path`，尝试删除对应文件，失败忽略（留给定时兜底）。
6. 变更后写回 store（`history` 键）。返回新/更新后的条目。

### 5.3 存储

- tauri-plugin-store，单文件双键：`history`（`ClipboardEntry[]`，最新在前）、`maxEntries`（`number`，默认 64，范围 1~1024）。
- store 文件位于应用数据目录（BaseDirectory::AppData）下，文件名 `clipboard.json`；实现时通过 `StoreExt` 以绝对路径定位，保证与图片目录同域。
- store 实例由插件缓存复用，service 通过持久化抽象访问（便于脱离 Tauri 测试）。

### 5.4 图片目录与清理

- 图片本体由插件保存于 `app_data_dir/tauri-plugin-clipboard-x/images`（插件默认路径，`readImage` 不传 `save_path`），文件名为内容哈希 `.png`。
- **前端显示**：通过 `convertFileSrc(path)` 生成 asset URL，依赖 `tauri.conf.json` 的 `security.assetProtocol`（enable=true，scope 覆盖 `$APPDATA/tauri-plugin-clipboard-x/images/**`）；`<img>` 加载失败（如协议未命中）时前端回退为占位文案，不显示裂图。
- **即时淘汰**（见 5.2-5）：超限同步删最旧条目及其图片。
- **定时兜底清理**：前端 `setInterval`（固定 5 分钟，不暴露设置项）→ invoke `cleanupOrphanImages`：扫描图片目录全部 `.png`，收集存活条目 `image.path` 集合，删除差集文件，返回 `removed`。用于弥合即时淘汰失败（文件占用、异常中断等）造成的不一致。
- **图片缺失**：`getClipboardHistory` 返回时对每条 `image` 检查文件存在性，生成派生标记 `missing`（不持久化）；条目保留不删除，前端显示占位。

### 5.5 writeClipboardEntry（回写）

按条目内容字段优先写回原始格式：

1. 有 `html` → 插件 `writeHTML(text?, html)`（纯文本回退）
2. 否则有 `rtf` → 插件 `writeRTF(text?, rtf)`
3. 否则有 `text` → 插件 `writeText`
4. 否则有 `image` 且文件存在 → 插件 `writeImage(path)`
5. 否则有 `files` → 插件 `writeFiles(paths)`
6. 无任何内容字段 → 不写回，返回成功（防御分支；正常流程不会出现）

注意：按项目内既有经验，本插件回写**不触发**剪贴板变化事件（即不回环到监听）；此行为可能随插件版本变化，实现后实测验证，若触发则去重逻辑自然置顶，无害。

### 5.6 deleteClipboardEntry / clearClipboardHistory

- 单条删除：删除条目；若有 `image.path` 尝试删除文件（失败忽略，留兜底）。
- 清空全部：清空 `history` 键 + 删除图片目录下全部 `.png`。

### 5.7 setMaxEntries / getMaxEntries

- 校验 `1 ≤ n ≤ 1024`，否则 `clipboard.invalid_max_entries`。
- 保存后若当前条数 > n，**立即截断**（复用即时淘汰逻辑），返回 `{maxEntries, evicted}`。

## 6. 破坏性影响

- 新功能，无既有接口破坏。
- 移除 `greet` 脚手架（后端命令 + 前端 App.tsx 演示）作为本功能签发的一部分。
- 依赖新增：Cargo.toml 加 `tauri-plugin-clipboard-x`、`tauri-plugin-store`；package.json 加 `tauri-plugin-clipboard-x-api`；capabilities 加 `clipboard-x:default`。

## 7. 未决问题

- [ ] 回写是否触发监听：按既有经验不触发，实现后实测（见 5.5）。
- [ ] 快速连续复制时事件到达可能读到"最新一次"内容而非事件时刻内容（Windows 剪贴板监视固有延迟），首版接受、README 注明。
- [ ] 大列表（1024 条富文本）每次全量序列化 store 的性能：README 注明已知限制，后续优化。
