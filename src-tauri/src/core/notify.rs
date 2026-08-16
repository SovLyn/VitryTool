//! 通用通知通道（0.2.8，横切基础）。
//!
//! 职责（契约 `docs/api/notify.md`）：
//! - 前端经 `notify` 命令提交通知 → 校验 → 广播 `app://notify` 事件到所有窗口；
//! - 后端内部站点（托盘开关失败、快捷键注册失败、lan-sync 节点错误等）经
//!   `notify_app` 直接广播——**只 emit 不阻塞**，emit 失败仅记日志；
//! - 负载为结构化 `level + code + params`，不含界面文案（前端渲染时 i18n 翻译，
//!   符合「后端不输出界面文案」铁律）。
//!
//! 设计约束（商讨 Q2/Q6）：不分配 id、不节流、不持久化——去重/折叠/关闭是前端
//! toast 的职责；后端 5 个站点均为用户一次性动作，无轰炸风险。

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::core::error::ApiError;

/// 通知事件名（后端 → 前端，广播到所有窗口）。
pub const NOTIFY_EVENT: &str = "app://notify";

/// 错误码（契约第 4 节）：通知参数非法。
const ERR_INVALID: &str = "notify.invalid";

/// 通知级别（序列化为小写字符串，契约 5.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyLevel {
    Success,
    Error,
    Warning,
    Info,
}

/// 通知负载（命令入参与事件载荷同构，契约第 3 节）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyPayload {
    pub level: NotifyLevel,
    /// 稳定 i18n 键或后端错误码（前端解析翻译，契约 5.4）。
    pub code: String,
    /// 插值参数（可选）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Map<String, serde_json::Value>>,
}

/// 解析 level 字符串 → 枚举（未知值返回 None，命令据此返回 `notify.invalid`）。
fn parse_level(s: &str) -> Option<NotifyLevel> {
    match s {
        "success" => Some(NotifyLevel::Success),
        "error" => Some(NotifyLevel::Error),
        "warning" => Some(NotifyLevel::Warning),
        "info" => Some(NotifyLevel::Info),
        _ => None,
    }
}

/// 校验通知参数（纯函数，供命令与 dt 复用）。
fn validate(level: &str, code: &str) -> Result<NotifyLevel, ApiError> {
    let level = parse_level(level)
        .ok_or_else(|| ApiError::new(ERR_INVALID, format!("unknown notify level: {level}")))?;
    if code.trim().is_empty() {
        return Err(ApiError::new(ERR_INVALID, "notify code must be non-empty"));
    }
    Ok(level)
}

/// 提交通知（契约 2）：校验 level/code，通过后广播到所有窗口。
///
/// 前端 fire-and-forget；emit 失败仅记日志，不阻塞主流程。
#[tauri::command]
pub fn notify(
    app: AppHandle,
    level: String,
    code: String,
    params: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<(), ApiError> {
    let level = validate(&level, &code)?;
    emit_notify(
        &app,
        NotifyPayload {
            level,
            code,
            params,
        },
    );
    Ok(())
}

/// 后端内部站点直发通知（只 emit 不阻塞，失败仅记日志）。
pub fn notify_app(app: &AppHandle, level: NotifyLevel, code: &str) {
    emit_notify(
        app,
        NotifyPayload {
            level,
            code: code.to_string(),
            params: None,
        },
    );
}

fn emit_notify(app: &AppHandle, payload: NotifyPayload) {
    if let Err(e) = app.emit(NOTIFY_EVENT, &payload) {
        log::warn!("notify: emit failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_level_accepts_all_four() {
        assert_eq!(parse_level("success"), Some(NotifyLevel::Success));
        assert_eq!(parse_level("error"), Some(NotifyLevel::Error));
        assert_eq!(parse_level("warning"), Some(NotifyLevel::Warning));
        assert_eq!(parse_level("info"), Some(NotifyLevel::Info));
    }

    #[test]
    fn parse_level_rejects_unknown() {
        assert_eq!(parse_level("fatal"), None);
        assert_eq!(parse_level("Success"), None); // 大小写敏感
        assert_eq!(parse_level(""), None);
    }

    #[test]
    fn validate_accepts_valid() {
        assert_eq!(
            validate("error", "lan.peer_node_error"),
            Ok(NotifyLevel::Error)
        );
        assert_eq!(validate("info", "lanSync.items"), Ok(NotifyLevel::Info));
    }

    #[test]
    fn validate_rejects_unknown_level() {
        let err = validate("fatal", "a.b").unwrap_err();
        assert_eq!(err.code, "notify.invalid");
    }

    #[test]
    fn validate_rejects_empty_or_whitespace_code() {
        assert_eq!(validate("error", "").unwrap_err().code, "notify.invalid");
        assert_eq!(validate("error", "   ").unwrap_err().code, "notify.invalid");
    }

    #[test]
    fn payload_serializes_level_lowercase() {
        let payload = NotifyPayload {
            level: NotifyLevel::Error,
            code: "lan.peer_node_error".into(),
            params: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["level"], "error");
        assert_eq!(json["code"], "lan.peer_node_error");
        assert!(json.get("params").is_none()); // None 不序列化
    }

    #[test]
    fn payload_serializes_params() {
        let mut params = serde_json::Map::new();
        params.insert("count".into(), serde_json::json!(3));
        let payload = NotifyPayload {
            level: NotifyLevel::Info,
            code: "lanSync.items".into(),
            params: Some(params),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["params"]["count"], 3);
    }

    #[test]
    fn level_enum_serialization_is_stable() {
        // 契约 5.3 的四个值；防止未来误改枚举破坏序列化契约
        assert_eq!(
            serde_json::to_string(&NotifyLevel::Success).unwrap(),
            "\"success\""
        );
        assert_eq!(
            serde_json::to_string(&NotifyLevel::Error).unwrap(),
            "\"error\""
        );
        assert_eq!(
            serde_json::to_string(&NotifyLevel::Warning).unwrap(),
            "\"warning\""
        );
        assert_eq!(
            serde_json::to_string(&NotifyLevel::Info).unwrap(),
            "\"info\""
        );
    }
}
