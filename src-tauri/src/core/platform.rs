//! 平台识别与移动端平台差异（契约 `docs/api/mobile.md`）。
//!
//! 职责：
//! - `getPlatformInfo` 命令：平台识别（`isMobile` / `platform` / 全局快捷键能力），
//!   前端功能隔离的唯一依据（契约 mobile 5.1）；
//! - 系统剪贴板写入的平台分发：桌面 clipboard-x / 移动 clipboard-manager（契约 mobile 5.2）；
//! - 移动端「可写纯文本」提取：text → html 剥标签 → imageMeta 占位（契约 mobile 5.2）；
//! - 全局快捷键能力判定：0.2.3 从 quick_paste 迁移至此（core 自包含，不依赖功能域，
//!   见 `core/mod.rs` 顶部注释；quick_paste 的 `getHotkeyCapability` 命令改为调用本模块）。

use serde::Serialize;

/// 当前是否移动平台（编译期判定，`desktop`/`mobile` cfg alias 由 tauri-build 注入）。
pub fn is_mobile() -> bool {
    cfg!(any(target_os = "android", target_os = "ios"))
}

/// 平台名（与契约 mobile 3 的枚举一致）。
pub fn platform_name() -> &'static str {
    if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// `getPlatformInfo` 响应（契约 mobile 3）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub is_mobile: bool,
    pub platform: &'static str,
    pub hotkey_capability: HotkeyCapability,
}

/// 全局快捷键能力（契约 quick-paste 5.8 / mobile 5.1）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyCapability {
    /// 移动端恒为 false（无全局快捷键概念）。
    pub supported: bool,
}

/// `getPlatformInfo` 命令（契约 mobile 2）。
#[tauri::command]
pub fn get_platform_info() -> PlatformInfo {
    let supported = global_shortcut_supported(
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("GDK_BACKEND").ok().as_deref(),
    );
    log::debug!(
        "get_platform_info: mobile={} platform={} hotkeySupported={supported}",
        is_mobile(),
        platform_name()
    );
    PlatformInfo {
        is_mobile: is_mobile(),
        platform: platform_name(),
        hotkey_capability: HotkeyCapability { supported },
    }
}

/// 全局快捷键能力判定（契约 quick-paste 5.8，0.2.3 逻辑迁入）。
///
/// Linux 下 `tauri-plugin-global-shortcut` 底层 `global-hotkey` 仅实现 X11 后端
/// （`XGrabKey`）：Wayland 会话中 GTK 窗口为原生 Wayland，键盘事件不经过 X server，
/// 快捷键注册「成功」但按下永不触发。仅当 GTK 显式强制 X11 后端（`GDK_BACKEND`
/// 含 `x11`，经 XWayland 运行）时才可能生效，故此时判定为支持。
///
/// 参数为注入的环境变量值（`XDG_SESSION_TYPE` / `WAYLAND_DISPLAY` / `GDK_BACKEND`），
/// 便于脱离 Tauri 运行时单元测试。
pub fn global_shortcut_supported(
    session_type: Option<&str>,
    wayland_display: Option<&str>,
    gdk_backend: Option<&str>,
) -> bool {
    if is_mobile() {
        // 移动端无全局快捷键概念（契约 mobile 5.1）
        return false;
    }
    let session = session_type.unwrap_or("").to_ascii_lowercase();
    // Wayland 会话：XDG_SESSION_TYPE=wayland，或该变量缺失但存在 WAYLAND_DISPLAY
    let on_wayland = session == "wayland" || (session.is_empty() && wayland_display.is_some());
    if !on_wayland {
        return true;
    }
    // Wayland 会话下仅当显式强制 X11 后端（GDK_BACKEND 含 x11，走 XWayland）才可能生效
    gdk_backend
        .unwrap_or("")
        .split([',', ' '])
        .any(|p| p.eq_ignore_ascii_case("x11"))
}

/// 写纯文本到系统剪贴板（契约 mobile 5.2 平台分发）。
///
/// 桌面：clipboard-x（异步，与应用其余剪贴板读写同一插件）；
/// 移动：clipboard-manager（同步 API，见 `write_text_plain_sync`）。
/// 桌面编译时无调用方（桌面回写走各命令内的格式写路径），`dead_code` 属预期。
#[cfg_attr(desktop, allow(dead_code))]
pub async fn write_text_plain(_app: &tauri::AppHandle, text: String) -> Result<(), String> {
    #[cfg(desktop)]
    {
        tauri_plugin_clipboard_x::write_text(text)
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(mobile)]
    {
        write_text_plain_sync(_app, text)
    }
}

/// 移动端同步写纯文本（clipboard-manager 同步 API；移动端写剪贴板 + 显式入历史的
/// 同步链路用，见 clipboard_history 的 `write_text_and_record`）。
#[cfg(mobile)]
pub fn write_text_plain_sync(app: &tauri::AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

/// 移动端可写纯文本提取（契约 mobile 5.2）：
///
/// - 有 `text`（非空）→ 原文；
/// - 否则有 `html` → 剥标签（`strip_html`）；
/// - 否则 `image_placeholder`（调用方按 imageMeta / 图片字段构造的占位文本）；
/// - 仅含文件路径 → `None`（不可写：前端禁用入口 + 后端兜底错误码
///   `clipboard.write_unsupported`）。
pub fn mobile_writable_text(
    text: Option<&str>,
    html: Option<&str>,
    image_placeholder: Option<String>,
) -> Option<String> {
    if let Some(t) = text {
        if !t.trim().is_empty() {
            return Some(t.to_string());
        }
    }
    if let Some(h) = html {
        let stripped = strip_html(h);
        if !stripped.trim().is_empty() {
            return Some(stripped);
        }
    }
    image_placeholder
}

/// 剥 HTML 标签并解码常见实体，得到可读纯文本（契约 mobile 5.2）。
///
/// 规则：跳过 `<script>` / `<style>` 内容；块级标签（p/div/li/br/tr/块标题等）后补换行；
/// 解码常见实体（`&amp;` `&lt;` `&gt;` `&quot;` `&#39;` `&apos;` `&nbsp;`）。
/// 非完整 HTML 解析器（不处理嵌套/属性转义边界），满足「移动端写剪贴板纯文本」场景即可。
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    let mut skipping = false; // script/style 块内：内容不输出，只追踪结束标签

    for ch in html.chars() {
        if skipping {
            // 只从 '<' 开始累积标签（避免内容字符污染 tag；内容中的 `<b>` 之类
            // 会累积到自身的 '>' 后按非结束标签清空，不影响后续 `</script>` 识别）
            if ch == '<' {
                tag.clear();
                tag.push(ch);
            } else if !tag.is_empty() {
                tag.push(ch);
                if ch == '>' {
                    let t = tag.to_ascii_lowercase();
                    if t.starts_with("</script") || t.starts_with("</style") {
                        skipping = false;
                    }
                    tag.clear();
                }
            }
            continue;
        }
        if in_tag {
            tag.push(ch);
            if ch == '>' {
                let t = tag.to_ascii_lowercase();
                let inner = t.trim_start_matches('<').trim_end_matches('>');
                let name = inner
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if name == "script" || name == "style" {
                    skipping = true; // 跳过其内容（含结束标签，见上分支）
                } else if matches!(
                    name,
                    "br" | "p"
                        | "div"
                        | "li"
                        | "tr"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "blockquote"
                        | "section"
                        | "table"
                        | "ul"
                        | "ol"
                        | "hr"
                ) {
                    out.push('\n');
                }
                in_tag = false;
                tag.clear();
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            tag.clear();
            tag.push('<');
        } else {
            out.push(ch);
        }
    }
    // 先解码实体（`&nbsp;` → 空格），再规整空白（trim 首尾 + 合并空行）
    collapse_blank_lines(&decode_entities(&out))
}

/// 解码常见 HTML 实体。
fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// 空行规整：连续 2 个以上换行压成 1 个；首尾空白去掉。
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newline_count = 0u32;
    for ch in s.chars() {
        if ch == '\n' {
            newline_count += 1;
            if newline_count <= 1 {
                out.push('\n');
            }
        } else {
            newline_count = 0;
            out.push(ch);
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_name_is_known() {
        // 编译期常量：当前测试平台一定属于已知枚举
        assert!(matches!(
            platform_name(),
            "windows" | "macos" | "linux" | "android" | "ios"
        ));
    }

    #[test]
    fn mobile_flag_matches_platform() {
        // is_mobile 与 platform_name 自洽（编译期同一 cfg）
        assert_eq!(is_mobile(), matches!(platform_name(), "android" | "ios"));
    }

    #[test]
    fn shortcut_supported_on_non_wayland_sessions() {
        // 桌面行为回归（0.2.3 测试迁入）；移动端下恒 false 由下一条覆盖
        if is_mobile() {
            return;
        }
        // X11 会话
        assert!(global_shortcut_supported(
            Some("x11"),
            Some("wayland-0"),
            None
        ));
        // 会话变量缺失且无 WAYLAND_DISPLAY（Windows / macOS / 未知环境）
        assert!(global_shortcut_supported(None, None, None));
        // 大小写不敏感
        assert!(global_shortcut_supported(
            Some("X11"),
            Some("wayland-0"),
            None
        ));
    }

    #[test]
    fn shortcut_unsupported_on_wayland_by_default() {
        if is_mobile() {
            return;
        }
        assert!(!global_shortcut_supported(
            Some("wayland"),
            Some("wayland-0"),
            None
        ));
        assert!(!global_shortcut_supported(
            Some("wayland"),
            Some("wayland-0"),
            Some("wayland")
        ));
        assert!(!global_shortcut_supported(None, Some("wayland-0"), None));
        assert!(!global_shortcut_supported(
            Some(""),
            Some("wayland-0"),
            None
        ));
    }

    #[test]
    fn shortcut_supported_on_wayland_with_forced_x11_backend() {
        if is_mobile() {
            return;
        }
        assert!(global_shortcut_supported(
            Some("wayland"),
            Some("wayland-0"),
            Some("x11")
        ));
        assert!(global_shortcut_supported(
            Some("wayland"),
            Some("wayland-0"),
            Some("x11,wayland")
        ));
        assert!(global_shortcut_supported(
            Some("wayland"),
            Some("wayland-0"),
            Some(" wayland , x11 ")
        ));
        assert!(global_shortcut_supported(
            Some("wayland"),
            Some("wayland-0"),
            Some("X11")
        ));
    }

    #[test]
    fn strip_html_removes_tags_and_entities() {
        assert_eq!(
            strip_html("<p>Hello <b>world</b> &amp; friends</p>"),
            "Hello world & friends"
        );
        assert_eq!(
            strip_html("a &lt; b &gt; c &quot;d&quot;"),
            "a < b > c \"d\""
        );
        assert_eq!(strip_html("x&#39;y&apos;z"), "x'y'z");
        assert_eq!(strip_html("&nbsp;leading"), "leading");
    }

    #[test]
    fn strip_html_skips_script_and_style_blocks() {
        // script/style 内容（含嵌套标签与 '>' 字符）被整体跳过；段落间空行压成单换行
        assert_eq!(
            strip_html(
                "<p>keep</p><script>var x = '<b>nope</b>';</script><style>.a{}</style><p>end</p>"
            ),
            "keep\nend"
        );
    }

    #[test]
    fn strip_html_adds_block_breaks() {
        assert_eq!(strip_html("<div>one</div><div>two</div>"), "one\ntwo");
    }

    #[test]
    fn mobile_writable_text_prefers_text_then_html_then_placeholder() {
        assert_eq!(
            mobile_writable_text(Some(" hi "), Some("<p>x</p>"), None),
            Some(" hi ".to_string())
        );
        assert_eq!(
            mobile_writable_text(None, Some("<p>x</p>"), None),
            Some("x".to_string())
        );
        assert_eq!(
            mobile_writable_text(None, None, Some("[图片] a.png (1x2)".to_string())),
            Some("[图片] a.png (1x2)".to_string())
        );
        // 空 text / 纯标签 html 也回退
        assert_eq!(
            mobile_writable_text(Some("  "), Some("<b></b>"), None),
            None
        );
        // 仅文件路径（text/html/占位皆无）→ 不可写
        assert_eq!(mobile_writable_text(None, None, None), None);
    }
}
