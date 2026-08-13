//! 全局状态。

/// 应用全局状态。
///
/// 所有功能共享的状态挂载在这里；功能私有的状态放在各自 mod 内。
/// 剪贴板历史（首个功能）通过命令内 StoreBackend 直接访问 store，无需挂载共享状态。
#[derive(Debug, Default)]
pub struct AppState {
    // 预留：后续功能需要共享状态时在此挂载（如设置、会话等）。
}

impl AppState {
    /// 创建全局状态实例。
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_constructible() {
        let state = AppState::new();
        // 骨架期仅验证可构造；首个功能落地后按需扩展断言。
        let _ = state;
    }
}
