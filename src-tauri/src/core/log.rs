//! 日志初始化（横切基础）。
//!
//! 基于 `log` 门面 + `tauri-plugin-log`（官方后端）。使用方式：在任意后端代码中
//! 直接调用 `log::trace! / debug! / info! / warn! / error!` 宏即可，无需关心输出目标。
//!
//! ## 级别与目标策略（约定）
//!
//! - **开发（debug_assertions）**：`Debug` 级输出到终端，便于调试；系统 crate
//!   （`tauri`、`tauri_plugin_*`、`libp2p*`、`quinn*`、`yamux`、`multistream_select`、`tracing`）
//!   压到 `Info`——libp2p 的 Trace/Debug（swarm 轮询、心跳、流协商）噪音过大，测试时只看本项目日志。
//!   本项目自身日志保持 Debug（`log::debug!` 及以下不输出，`info` 正常）。
//! - **发布（release）**：`Error` 级写入日志文件（应用日志目录，见 `app.log_dir()`），
//!   单文件 5 MiB 大小轮转，仅保留最近一份（`KeepOne`）。
//!
//! ## 隐私约束
//!
//! 剪贴板历史可能包含密码等敏感内容：**日志中只记录元数据**（条目 id、内容类型、
//! 长度、文件路径、操作结果），绝不记录剪贴板明文内容。

use log::LevelFilter;
use tauri::Wry;
use tauri_plugin_log::{Builder, Target, TargetKind};

/// 发布版日志文件基名（`tauri-plugin-log` 会在其下追加序号/日期）。
#[cfg(not(debug_assertions))]
const RELEASE_LOG_FILE: &str = "vitrytool";
/// 发布版日志单文件大小上限（字节）。
#[cfg(not(debug_assertions))]
const RELEASE_MAX_FILE_SIZE: u128 = 5 * 1024 * 1024;
/// 开发版日志文件基名（Trace 同时落盘，便于排查问题）。
#[cfg(debug_assertions)]
const DEV_LOG_FILE: &str = "vitrytool-dev";

/// 返回按构建模式配置好的日志插件。
///
/// 开发 / 发布分别通过 `#[cfg]` 分支选择级别与输出目标。
pub fn plugin() -> tauri::plugin::TauriPlugin<Wry> {
    #[cfg(debug_assertions)]
    let builder = Builder::new()
        .level(LevelFilter::Debug)
        // 压系统 crate 噪音（libp2p 的 Trace/Debug 心跳与流协商在测试时过于嘈杂）
        .level_for("tauri", LevelFilter::Info)
        .level_for("tauri_plugin_clipboard_x", LevelFilter::Info)
        .level_for("tauri_plugin_store", LevelFilter::Info)
        .level_for("tauri_plugin_opener", LevelFilter::Info)
        // 统一 filter：
        // - tracing→log 桥的 span 生命周期记录（target `tracing::span`）直接丢弃；
        // - libp2p 全部子 crate（libp2p_tcp/quic/mdns/core/…）的 Debug/Trace 丢弃，Info 保留
        //   （level_for 为精确模块匹配，无法覆盖子 crate，故在此按 target 前缀统一处理）。
        .filter(|metadata| {
            let t = metadata.target();
            if t.starts_with("tracing::span") {
                return false;
            }
            // log::Level 按严重度降序（Error<Warn<Info<Debug<Trace），
            // 故丢弃 Debug 及以下（>= Debug）即丢弃 libp2p 的 Debug/Trace。
            if t.starts_with("libp2p") && metadata.level() >= log::Level::Debug {
                return false;
            }
            true
        })
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::LogDir {
                file_name: Some(DEV_LOG_FILE.into()),
            }),
        ]);

    #[cfg(not(debug_assertions))]
    let builder = Builder::new()
        .level(LevelFilter::Error)
        .targets([Target::new(TargetKind::LogDir {
            file_name: Some(RELEASE_LOG_FILE.into()),
        })])
        .max_file_size(RELEASE_MAX_FILE_SIZE)
        .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne);

    builder.build()
}
