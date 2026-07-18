//! 全平台共享类型（tmux pane / window / session id）。
//!
//! `parse` 实现留在 [`crate::core::tmux::protocol`]，以便复用 `ProtocolError`、
//! 避免 types ↔ protocol 循环依赖。

/// %output / %extended-output / %pane-mode-changed 里的 pane id（`@N`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

impl PaneId {
    pub fn as_str(self) -> String {
        format!("@{}", self.0)
    }
}

/// window id（`@N`），与 pane id 同形式，靠字段位置区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u32);

impl WindowId {
    pub fn as_str(self) -> String {
        format!("@{}", self.0)
    }
}

/// session id（`$N`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u32);

impl SessionId {
    pub fn as_str(self) -> String {
        format!("${}", self.0)
    }
}
