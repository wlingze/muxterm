//! WorkspaceId：稳定的工作区标识，不是 tmux `$N`。
//!
//! 由连接身份构成（transport / alias / session / runtime / path），
//! 与今天 platform 的 `ConnectionKey` 同构；W2 的池按它复用。

/// 稳定字符串标识：`transport/alias/session/runtime/path`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceId {
    pub transport: String,
    pub alias: Option<String>,
    pub session: String,
    pub runtime: String,
    pub path: String,
}

impl WorkspaceId {
    /// 从连接身份字段构造稳定 id。
    pub fn new(
        transport: &str,
        alias: Option<&str>,
        session: &str,
        runtime: &str,
        path: &str,
    ) -> Self {
        Self {
            transport: transport.to_string(),
            alias: alias.map(ToOwned::to_owned),
            session: session.to_string(),
            runtime: runtime.to_string(),
            path: path.to_string(),
        }
    }

    /// 稳定显示形式（alias 为空时留空段）。
    pub fn as_str(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            self.transport,
            self.alias.as_deref().unwrap_or_default(),
            self.session,
            self.runtime,
            self.path
        )
    }
}

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_id_equality() {
        let a = WorkspaceId::new("local", None, "demo", "tmux", "");
        let same = WorkspaceId::new("local", None, "demo", "tmux", "");
        let other = WorkspaceId::new("local", None, "other", "tmux", "");
        assert_eq!(a, same);
        assert_ne!(a, other);
        // 不同 transport / alias / runtime / path 也构成不同身份。
        assert_ne!(
            a,
            WorkspaceId::new("ssh", Some("ryzen"), "demo", "tmux", "")
        );
        assert_ne!(a, WorkspaceId::new("local", None, "demo", "shell", ""));
        assert_ne!(a, WorkspaceId::new("local", None, "demo", "tmux", "/x"));
    }

    #[test]
    fn workspace_id_display_is_stable_string() {
        let local = WorkspaceId::new("local", None, "demo", "tmux", "");
        assert_eq!(local.as_str(), "local//demo/tmux/");
        let ssh = WorkspaceId::new("ssh", Some("ryzen"), "legion", "tmux", "~/work");
        assert_eq!(ssh.as_str(), "ssh/ryzen/legion/tmux/~/work");
    }
}
