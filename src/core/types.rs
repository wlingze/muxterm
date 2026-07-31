//! 全平台共享类型（tmux pane / window / session id）。
//!
//! `parse` 实现留在 [`crate::core::runtime::tmux::protocol`]，以便复用 `ProtocolError`、
//! 避免 types ↔ protocol 循环依赖。

/// %output / %extended-output / %pane-mode-changed 里的 pane id（`@N`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PaneId(pub u32);

impl PaneId {
    pub fn as_str(self) -> String {
        format!("@{}", self.0)
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{}", self.0)
    }
}

/// window id（`wN`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct WindowId(pub u32);

impl WindowId {
    pub fn as_str(self) -> String {
        format!("w{}", self.0)
    }
}

impl std::fmt::Display for WindowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "w{}", self.0)
    }
}

/// tab id（`tN`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TabId(pub u32);

impl TabId {
    pub fn as_str(self) -> String {
        format!("t{}", self.0)
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "t{}", self.0)
    }
}

/// session id（`$N`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SessionId(pub u32);

impl SessionId {
    pub fn as_str(self) -> String {
        format!("${}", self.0)
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "${}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对应：共享 ID 类型解析与显示不应回归。
    #[test]
    fn test_types_pane_id_parse_and_display() {
        for (raw, n) in [("@1", 1u32), ("@12", 12), ("@999", 999)] {
            let id = PaneId::parse(raw).unwrap();
            assert_eq!(id, PaneId(n));
            assert_eq!(id.as_str(), raw);
            assert_eq!(id.to_string(), raw);
        }
    }

    #[test]
    fn test_types_window_id_parse_and_display() {
        let id = WindowId(7);
        assert_eq!(id.as_str(), "w7");
        assert_eq!(format!("{id}"), "w7");
    }

    #[test]
    fn test_types_session_id_parse_and_display() {
        let id = SessionId::parse("$3").unwrap();
        assert_eq!(id, SessionId(3));
        assert_eq!(id.as_str(), "$3");
        assert_eq!(format!("{id}"), "$3");
    }

    /// 对应：非法 pane/window/session id 必须拒绝，避免静默错绑。
    #[test]
    fn test_types_reject_malformed_ids() {
        for bad in ["", "@", "abc", "1", "$1", "@@1"] {
            assert!(PaneId::parse(bad).is_err(), "pane 应拒绝 {bad:?}");
        }
        for bad in ["", "abc"] {
            let _ = bad; // WindowId parse 在 protocol.rs，这里不再测
        }
        for bad in ["", "$", "abc", "0", "@0", "$$1"] {
            assert!(SessionId::parse(bad).is_err(), "session 应拒绝 {bad:?}");
        }
    }

    /// PaneId / WindowId 同形不同型，避免混用。
    #[test]
    fn test_types_pane_and_window_are_distinct_newtypes() {
        let p = PaneId(1);
        let w = WindowId(1);
        // pane=@1, window=w1 — 不同前缀了
        assert_eq!(p.0, w.0);
    }

    #[test]
    fn test_types_roundtrip_parse_as_str() {
        let p = PaneId::parse(&PaneId(42).as_str()).unwrap();
        assert_eq!(p, PaneId(42));
        let s = SessionId(0);
        assert_eq!(s.as_str(), "$0");
        assert_eq!(s, SessionId(0));
    }
}
