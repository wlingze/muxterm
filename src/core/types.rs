//! 全平台共享产品 ID：PaneId / TabId。
//!
//! tmux 的 `$N` / `@N` 只留在 `runtime/tmux`（`TmuxSessionId` / 映射到 `TabId`），
//! 产品层没有 Session / Window。`parse` 实现留在
//! [`crate::core::runtime::tmux::protocol`]，以便复用 `ProtocolError`。

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

    /// 对应：非法 pane/window/session id 必须拒绝，避免静默错绑。
    /// F6：pane id 接受 `@N` / `%N` / `N`（tmux 3.3+），非数字仍拒绝。
    #[test]
    fn test_types_reject_malformed_ids() {
        for bad in ["", "@", "abc", "$1", "@@1", "%x"] {
            assert!(PaneId::parse(bad).is_err(), "pane 应拒绝 {bad:?}");
        }
    }

    #[test]
    fn test_types_roundtrip_parse_as_str() {
        let p = PaneId::parse(&PaneId(42).as_str()).unwrap();
        assert_eq!(p, PaneId(42));
    }
}
