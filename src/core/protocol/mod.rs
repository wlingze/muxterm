//! Core protocol layer: model + terminal + ffi (C ABI).

pub mod terminal;

// The DTO source files remain in their legacy location for this incremental
// migration, but these declarations make protocol the owning module tree.
#[path = "../model/layout.rs"]
pub mod layout;
#[path = "../model/state.rs"]
pub mod state;
#[path = "../model/task.rs"]
pub mod task;

#[cfg(feature = "ffi")]
pub mod ffi;

/// Stable protocol namespaces.
///
/// The implementation files are still kept under the legacy `model` module
/// during the incremental migration, but callers use this boundary so the
/// eventual crate split does not change every Runtime/frontend import again.

/// Runtime 能力声明。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capability {
    pub can_attach: bool,
    pub can_discover: bool,
    pub can_display_message: bool,
}

impl Capability {
    pub fn shell() -> Self {
        Self {
            can_attach: false,
            can_discover: false,
            can_display_message: false,
        }
    }

    pub fn tmux() -> Self {
        Self {
            can_attach: true,
            can_discover: true,
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
        assert!(!s.can_discover);
        assert!(!s.can_display_message);

        let t = Capability::tmux();
        assert!(t.can_attach);
        assert!(t.can_discover);
        assert!(t.can_display_message);
    }

    #[test]
    fn capability_serializable() {
        let c = Capability::tmux();
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"can_attach\":true"));
    }
}
