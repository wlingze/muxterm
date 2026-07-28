//! Core Protocol 层：muxterm 对外稳定的 Session → Window → Tab → Pane 模型。
//!
//! 本模块是 Core Protocol 的门面（facade），re-export 现有 `core::model` 中的
//! 纯数据类型，使其在语义上归属于 Core Protocol 层。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §3。
//!
//! Task / StateChange / Snapshot / ID 规则 / 能力差异 / 错误是稳定接口。
//! 新增 Runtime 不修改这些类型；新增 Transport 不修改这些类型。

// 从 core::model re-export（facade）
/// Re-export layout module (unused warnings suppressed for facade)
#[allow(unused_imports)]
pub use crate::core::model::layout;
#[allow(unused_imports)]
pub use crate::core::model::state;
#[allow(unused_imports)]
pub use crate::core::model::task;

/// Runtime 能力声明：描述某 Runtime 支持的操作集合。
///
/// Core Protocol 定义「能做什么」，Runtime 声明「我能做哪些」。
/// 前端/CLI 可通过 capability 决定 UI 是否展示某些操作。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capability {
    /// 支持 attach/detach 语义（tmux 模式）。
    pub can_attach: bool,
    /// 支持跨 session 查询（list-sessions beyond own）。
    pub can_list_sessions: bool,
    /// 支持 display-message（tmux format 查询）。
    pub can_display_message: bool,
}

impl Capability {
    /// Shell 模式能力集。
    pub fn shell() -> Self {
        Self {
            can_attach: false,
            can_list_sessions: false,
            can_display_message: false,
        }
    }

    /// Tmux 模式能力集。
    pub fn tmux() -> Self {
        Self {
            can_attach: true,
            can_list_sessions: true,
            can_display_message: true,
        }
    }
}

/// Core Protocol 错误（库层）。
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("不支持的 Task: {0}")]
    UnsupportedTask(String),
    #[error("muxterm ID 不存在: {0}")]
    IdNotFound(String),
    #[error("Runtime 未连接")]
    NotConnected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_shell_vs_tmux() {
        let s = Capability::shell();
        assert!(!s.can_attach);
        assert!(!s.can_list_sessions);
        assert!(!s.can_display_message);

        let t = Capability::tmux();
        assert!(t.can_attach);
        assert!(t.can_list_sessions);
        assert!(t.can_display_message);
    }

    #[test]
    fn capability_serializable() {
        let c = Capability::tmux();
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"can_attach\":true"));
    }
}
