# 0001 剪贴板捕捉链路经前端事件驱动

剪贴板监听线程运行在 Rust（tauri-plugin-clipboard-x 内部），但插件把"剪贴板已变化"仅作为事件广播到 WebView，不向 Rust 注册业务回调。因此我们决定：业务命令（captureClipboard、cleanupOrphanImages 等）全部由前端事件/定时器发起，后端保持无自有时钟；前端只做触发器，数据读写与文件管理全部落在后端 service。备选方案（Rust 端自建 clipboard-rs 监听、业务直连、不经过 WebView）被否决——它会与插件监听冲突或重复，且破坏"前端集中 invoke 封装"的项目架构。

推论：窗口/WebView 存活是捕捉的前提；将来若做托盘常驻捕捉，需重新评估此决策（见 TODO）。
