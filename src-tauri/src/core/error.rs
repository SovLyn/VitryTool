//! 统一错误类型。

use serde::Serialize;

/// 结构化 API 错误：稳定错误码 + 兜底消息。
///
/// 错误码格式：`<功能域>.<错误名>`（全小写点分命名）。
/// 前端以错误码为 key 查本地化字典展示文案，后端消息仅作开发期兜底，
/// 见 `docs/api/README.md` 第 4、5 节。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    /// 稳定错误码，如 `lan_share.peer_not_found`
    pub code: String,
    /// 兜底消息（开发期详细，生产可省略）
    pub message: String,
}

impl ApiError {
    /// 构造错误。错误码使用 `<功能域>.<错误名>` 点分命名。
    ///
    /// ```
    /// use vitrytool_lib::core::error::ApiError;
    ///
    /// let err = ApiError::new("lan_share.peer_not_found", "peer not found");
    /// assert_eq!(err.code, "lan_share.peer_not_found");
    /// ```
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_uses_dot_naming() {
        let err = ApiError::new("lan_share.peer_not_found", "peer not found");
        assert_eq!(err.code, "lan_share.peer_not_found");
        assert!(err.code.contains('.'));
    }

    #[test]
    fn display_contains_code_and_message() {
        let err = ApiError::new("core.internal", "boom");
        let text = err.to_string();
        assert!(text.contains("core.internal"));
        assert!(text.contains("boom"));
    }
}
