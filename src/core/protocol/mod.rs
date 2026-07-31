//! Core protocol layer: model + terminal + ffi (C ABI).

pub mod terminal;

#[cfg(feature = "ffi")]
pub mod ffi;

/// Re-export model submodules for convenience.
#[allow(unused_imports)]
pub use crate::core::model::layout;
#[allow(unused_imports)]
pub use crate::core::model::state;
#[allow(unused_imports)]
pub use crate::core::model::task;

/// Runtime 能力声明。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capability {
    pub can_attach: bool,
    pub can_list_sessions: bool,
    pub can_display_message: bool,
}

impl Capability {
    pub fn shell() -> Self {
        Self {
            can_attach: false,
            can_list_sessions: false,
            can_display_message: false,
        }
    }

    pub fn tmux() -> Self {
        Self {
            can_attach: true,
            can_list_sessions: true,
            can_display_message: true,
        }
    }
}

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
