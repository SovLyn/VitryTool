//! 快速粘贴业务核心（纯逻辑，无 IO，可脱离 Tauri 上下文独立测试）。
//!
//! 本模块只处理全局快捷键字符串的**解析 / 规范化 / 校验**，
//! 行为契约见 `docs/api/quick-paste.md` 第 3、5.1、5.2 节。
//!
//! 约定：
//! - 存储与传输使用 tauri-plugin-global-shortcut（global-hotkey）标准格式，
//!   修饰键 + 主键，`+` 分隔，如 `CommandOrControl+Shift+K`；
//! - 规范化输出固定修饰键顺序：`CommandOrControl` → `Alt` → `Shift` → `Super`；
//! - 主键使用规范名：大写字母 / 数字 / `F1`-`F12` / `Space` / `Enter` / `Tab` 等。

use std::fmt;

/// 快捷键校验失败的分类（日志用；对外统一映射为 `quick_paste.invalid_hotkey`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyParseError {
    /// 缺少主键（如 `Ctrl+`）。
    MissingMainKey,
    /// 没有任何修饰键（裸键会拦截常规输入，禁止）。
    MissingModifier,
    /// 只有 Shift 修饰（`Shift+A` 会干扰文字输入，禁止）。
    ShiftOnly,
    /// 含无法识别 / 不支持的 token（修饰键或主键）。
    UnsupportedToken(String),
}

impl fmt::Display for HotkeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMainKey => write!(f, "hotkey has no main key"),
            Self::MissingModifier => write!(f, "hotkey needs at least one modifier (Ctrl/Alt/Win)"),
            Self::ShiftOnly => write!(f, "hotkey with only Shift modifier is not allowed"),
            Self::UnsupportedToken(t) => write!(f, "unsupported token: {t}"),
        }
    }
}

/// 规范输出中修饰键的顺序。
const MOD_ORDER: [&str; 4] = ["CommandOrControl", "Alt", "Shift", "Super"];

/// 解析单个修饰键 token；返回规范名，非法返回 `None`。
fn parse_modifier(token: &str) -> Option<&'static str> {
    match token.to_uppercase().as_str() {
        "CONTROL" | "CTRL" | "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL"
        | "CMDORCONTROL" => Some("CommandOrControl"),
        "ALT" | "OPTION" => Some("Alt"),
        "SHIFT" => Some("Shift"),
        "SUPER" | "COMMAND" | "CMD" => Some("Super"),
        _ => None,
    }
}

/// 解析单个主键 token；返回规范名，非法 / 不在白名单返回 `None`。
///
/// 白名单与录制组件（前端）保持一致：字母、数字、F1-F12、若干常用功能键。
fn parse_key(token: &str) -> Option<&'static str> {
    let upper = token.to_uppercase();
    match upper.as_str() {
        // 字母（解析器同时接受 `KeyX` 形式，规范化为字母本身）
        "A" | "KEYA" => Some("A"),
        "B" | "KEYB" => Some("B"),
        "C" | "KEYC" => Some("C"),
        "D" | "KEYD" => Some("D"),
        "E" | "KEYE" => Some("E"),
        "F" | "KEYF" => Some("F"),
        "G" | "KEYG" => Some("G"),
        "H" | "KEYH" => Some("H"),
        "I" | "KEYI" => Some("I"),
        "J" | "KEYJ" => Some("J"),
        "K" | "KEYK" => Some("K"),
        "L" | "KEYL" => Some("L"),
        "M" | "KEYM" => Some("M"),
        "N" | "KEYN" => Some("N"),
        "O" | "KEYO" => Some("O"),
        "P" | "KEYP" => Some("P"),
        "Q" | "KEYQ" => Some("Q"),
        "R" | "KEYR" => Some("R"),
        "S" | "KEYS" => Some("S"),
        "T" | "KEYT" => Some("T"),
        "U" | "KEYU" => Some("U"),
        "V" | "KEYV" => Some("V"),
        "W" | "KEYW" => Some("W"),
        "X" | "KEYX" => Some("X"),
        "Y" | "KEYY" => Some("Y"),
        "Z" | "KEYZ" => Some("Z"),
        // 数字
        "0" | "DIGIT0" => Some("0"),
        "1" | "DIGIT1" => Some("1"),
        "2" | "DIGIT2" => Some("2"),
        "3" | "DIGIT3" => Some("3"),
        "4" | "DIGIT4" => Some("4"),
        "5" | "DIGIT5" => Some("5"),
        "6" | "DIGIT6" => Some("6"),
        "7" | "DIGIT7" => Some("7"),
        "8" | "DIGIT8" => Some("8"),
        "9" | "DIGIT9" => Some("9"),
        // 功能键
        "F1" => Some("F1"),
        "F2" => Some("F2"),
        "F3" => Some("F3"),
        "F4" => Some("F4"),
        "F5" => Some("F5"),
        "F6" => Some("F6"),
        "F7" => Some("F7"),
        "F8" => Some("F8"),
        "F9" => Some("F9"),
        "F10" => Some("F10"),
        "F11" => Some("F11"),
        "F12" => Some("F12"),
        // 常用功能键
        "SPACE" => Some("Space"),
        "ENTER" | "RETURN" => Some("Enter"),
        "TAB" => Some("Tab"),
        "BACKSPACE" => Some("Backspace"),
        "DELETE" => Some("Delete"),
        "HOME" => Some("Home"),
        "END" => Some("End"),
        "PAGEUP" => Some("PageUp"),
        "PAGEDOWN" => Some("PageDown"),
        "INSERT" => Some("Insert"),
        "ARROWUP" | "UP" => Some("ArrowUp"),
        "ARROWDOWN" | "DOWN" => Some("ArrowDown"),
        "ARROWLEFT" | "LEFT" => Some("ArrowLeft"),
        "ARROWRIGHT" | "RIGHT" => Some("ArrowRight"),
        _ => None,
    }
}

/// 校验并规范化快捷键字符串（契约 5.1-③、5.2）。
///
/// 规则：
/// 1. 以 `+` 拆分 token，逐个识别；修饰键可多选、顺序任意，主键只能一个；
/// 2. 必须包含主键；
/// 3. 必须包含至少一个非 Shift 修饰键（`CommandOrControl` / `Alt` / `Super`）；
/// 4. 输出规范格式（修饰键固定顺序 + 主键规范名）。
pub fn normalize_hotkey(input: &str) -> Result<String, HotkeyParseError> {
    let mut mods: Vec<&'static str> = Vec::new();
    let mut key: Option<&'static str> = None;

    for raw in input.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            continue; // 容忍 `Ctrl + K` 中多余的空格
        }
        // 与 global-hotkey 解析器一致：主键之后不再接受任何 token（含修饰键）
        if key.is_some() {
            return Err(HotkeyParseError::UnsupportedToken(token.to_string()));
        }
        if let Some(mod_name) = parse_modifier(token) {
            if !mods.contains(&mod_name) {
                mods.push(mod_name);
            }
            continue;
        }
        key = Some(
            parse_key(token)
                .ok_or_else(|| HotkeyParseError::UnsupportedToken(token.to_string()))?,
        );
    }

    let key = key.ok_or(HotkeyParseError::MissingMainKey)?;
    if mods.is_empty() {
        return Err(HotkeyParseError::MissingModifier);
    }
    if mods.iter().all(|m| *m == "Shift") {
        return Err(HotkeyParseError::ShiftOnly);
    }

    // 规范顺序输出
    let mut parts: Vec<&str> = MOD_ORDER.iter().filter(|m| mods.contains(m)).copied().collect();
    parts.push(key);
    Ok(parts.join("+"))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn normalizes_common_shortcuts() {
        assert_eq!(normalize_hotkey("Ctrl+Shift+K").unwrap(), "CommandOrControl+Shift+K");
        assert_eq!(normalize_hotkey("shift+ctrl+k").unwrap(), "CommandOrControl+Shift+K");
        assert_eq!(normalize_hotkey("Alt+1").unwrap(), "Alt+1");
        assert_eq!(normalize_hotkey("Super+Space").unwrap(), "Super+Space");
        assert_eq!(normalize_hotkey("CmdOrCtrl+Shift+Alt+K").unwrap(), "CommandOrControl+Alt+Shift+K");
        assert_eq!(normalize_hotkey("Ctrl+F5").unwrap(), "CommandOrControl+F5");
        assert_eq!(normalize_hotkey("CTRL+KeyX").unwrap(), "CommandOrControl+X");
        assert_eq!(normalize_hotkey(" Control + Shift + ArrowUp ").unwrap(), "CommandOrControl+Shift+ArrowUp");
    }

    #[test]
    fn rejects_missing_modifier() {
        assert_eq!(normalize_hotkey("K"), Err(HotkeyParseError::MissingModifier));
        assert_eq!(normalize_hotkey("F1"), Err(HotkeyParseError::MissingModifier));
        assert_eq!(normalize_hotkey("1"), Err(HotkeyParseError::MissingModifier));
    }

    #[test]
    fn rejects_shift_only() {
        assert_eq!(normalize_hotkey("Shift+K"), Err(HotkeyParseError::ShiftOnly));
        assert_eq!(normalize_hotkey("Shift+Shift+A"), Err(HotkeyParseError::ShiftOnly));
    }

    #[test]
    fn rejects_missing_main_key() {
        assert_eq!(normalize_hotkey("Ctrl+"), Err(HotkeyParseError::MissingMainKey));
        assert_eq!(normalize_hotkey("Ctrl+Shift"), Err(HotkeyParseError::MissingMainKey));
    }

    #[test]
    fn rejects_unknown_tokens() {
        assert!(matches!(
            normalize_hotkey("Ctrl+Foo"),
            Err(HotkeyParseError::UnsupportedToken(_))
        ));
        // 两个主键
        assert!(matches!(
            normalize_hotkey("Ctrl+Shift+K+J"),
            Err(HotkeyParseError::UnsupportedToken(_))
        ));
        // 修饰键出现在主键之后（顺序错误）
        assert!(matches!(
            normalize_hotkey("Ctrl+K+Shift"),
            Err(HotkeyParseError::UnsupportedToken(_))
        ));
        // 大小写无关的 F13 超出白名单（避免误注册）
        assert!(matches!(
            normalize_hotkey("Ctrl+F13"),
            Err(HotkeyParseError::UnsupportedToken(_))
        ));
    }

    #[test]
    fn deduplicates_modifiers() {
        assert_eq!(normalize_hotkey("Ctrl+Ctrl+K").unwrap(), "CommandOrControl+K");
        assert_eq!(normalize_hotkey("Shift+Ctrl+Shift+K").unwrap(), "CommandOrControl+Shift+K");
    }

    #[test]
    fn supports_canonical_and_alternate_names() {
        assert_eq!(normalize_hotkey("CommandOrControl+Option+Super+T").unwrap(), "CommandOrControl+Alt+Super+T");
        assert_eq!(normalize_hotkey("Cmd+Enter").unwrap(), "Super+Enter");
        assert_eq!(normalize_hotkey("Ctrl+Down").unwrap(), "CommandOrControl+ArrowDown");
    }
}
