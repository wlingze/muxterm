//! Cmd-P 面板里的「现有 runtime」行。
//!
//! 对照 macOS `App/ConnectionDiscovery.swift` 的 `ExistingConnectionChoice`
//! 与 `UnifiedPanelController` 的 Existing 分区：Core 的 discovery 结果是
//! 唯一来源，前端只做投影，**不**在启动时手动拼一个 session 名再打开。
//!
//! `in_pool` 的候选已经在工作区池里（就是左侧那些已打开的 workspace），
//! 再列一次会让同一条 runtime 出现两遍。

use crate::frontend::gpui::model::quick_connect::TargetConfig;
use crate::frontend::gpui::model::target::{TargetRuntime, TargetTransport};
use crate::frontend::utils::corebridge::{ExistingCandidate, TmuxSessionEntry};

/// 一条可以直接 attach 的现有 runtime。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingRuntime {
    pub name: String,
    pub runtime: String,
    pub transport: String,
    /// SSH 别名；本地为空。
    pub target: String,
    pub session: String,
    pub socket: Option<String>,
    pub windows: u32,
    pub attached: bool,
    pub workspace_id: Option<String>,
    pub path: String,
}

impl ExistingRuntime {
    /// discovery 候选 → 面板行。已在池里的、没有 session 的都丢弃。
    pub fn from_candidate(candidate: &ExistingCandidate) -> Option<Self> {
        if candidate.in_pool {
            return None;
        }
        let session = candidate
            .session
            .clone()
            .filter(|session| !session.is_empty())
            .or_else(|| Some(candidate.name.clone()).filter(|name| !name.is_empty()))?;
        let runtime = if candidate.runtime.is_empty() {
            "tmux".to_string()
        } else {
            candidate.runtime.clone()
        };
        let transport = if candidate.transport.is_empty() {
            "local".to_string()
        } else {
            candidate.transport.clone()
        };
        Some(Self {
            name: candidate.name.clone(),
            runtime,
            transport,
            target: candidate.target.clone(),
            session,
            // 空 socket 与「没有专属 socket」等价，统一收敛成 None。
            socket: candidate.socket.clone().filter(|socket| !socket.is_empty()),
            windows: candidate.windows,
            attached: candidate.attached,
            workspace_id: candidate.workspace_id.clone(),
            path: String::new(),
        })
    }

    /// 隔离 socket 上的 session（`-L <name>`）。
    ///
    /// 对照 `ConnectionDiscovery.listExistingConnections`：默认 server 之外，
    /// 当前连接自己的 socket 也要合并进来，否则用 `-L` 起的 demo server
    /// 永远不会出现在面板里。
    pub fn from_tmux_session(entry: &TmuxSessionEntry, socket: Option<&str>) -> Self {
        Self {
            name: entry.name.clone(),
            runtime: "tmux".to_string(),
            transport: "local".to_string(),
            target: String::new(),
            session: entry.name.clone(),
            socket: socket.map(str::to_string),
            windows: entry.windows,
            attached: entry.attached,
            workspace_id: None,
            path: String::new(),
        }
    }

    /// 批量投影：保持 discovery 顺序，去掉已在池里的。
    pub fn collect(candidates: &[ExistingCandidate]) -> Vec<Self> {
        candidates.iter().filter_map(Self::from_candidate).collect()
    }

    /// 激活这一行时要交给 Catalog 的目标配置。
    ///
    /// `path` 留空：attach 已有 session 不需要工作目录。
    pub fn config(&self) -> TargetConfig {
        let transport = if self.transport == "local" || self.transport.is_empty() {
            TargetTransport::Local
        } else {
            TargetTransport::Ssh {
                name: self.target.clone(),
            }
        };
        TargetConfig {
            name: self.name.clone(),
            runtime: TargetRuntime::parse(&self.runtime).unwrap_or(TargetRuntime::Tmux),
            transport,
            path: self.path.clone(),
            session: Some(self.session.clone()),
            socket: self.socket.clone(),
            workspace_id: self.workspace_id.clone(),
            persisted_id: None,
        }
    }

    /// 面板次行：`tmux @ local · 3 windows · attached`。
    pub fn detail(&self) -> String {
        let host = if self.transport == "ssh" {
            &self.target
        } else {
            &self.transport
        };
        let mut parts = vec![format!("{} @ {}", self.runtime, host)];
        if !self.path.is_empty() {
            parts.push(self.path.clone());
        }
        if let Some(workspace) = &self.workspace_id {
            parts.push(format!("{} · {workspace}", self.session));
        }
        if self.windows > 0 {
            parts.push(format!("{} windows", self.windows));
        }
        if self.attached {
            parts.push("attached".to_string());
        }
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str) -> ExistingCandidate {
        ExistingCandidate {
            name: name.to_string(),
            id: name.to_string(),
            runtime: "tmux".to_string(),
            transport: "local".to_string(),
            target: String::new(),
            in_pool: false,
            windows: 2,
            attached: true,
            created: 0,
            namespace: None,
            session: Some(name.to_string()),
            socket: None,
            workspace_id: None,
        }
    }

    /// 对照 `UnifiedPanelExistingConnectionsE2ETests`：已打开的不再列 Existing。
    #[test]
    fn candidates_already_in_the_pool_are_skipped() {
        let mut pooled = candidate("demo");
        pooled.in_pool = true;
        let rows = ExistingRuntime::collect(&[candidate("ops"), pooled]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "ops");
    }

    /// 对照 `ExistingConnectionChoice.init(target:session:socket:)`：
    /// session 与 socket 一起固化进目标配置，激活时不再从当前 Workspace 反推。
    #[test]
    fn config_freezes_the_session_and_socket() {
        let mut entry = candidate("demo");
        entry.socket = Some("/tmp/tmux-1000/default".to_string());
        entry.windows = 3;
        let config = ExistingRuntime::from_candidate(&entry).unwrap().config();
        assert_eq!(config.session.as_deref(), Some("demo"));
        assert_eq!(config.socket.as_deref(), Some("/tmp/tmux-1000/default"));
        assert_eq!(config.runtime, TargetRuntime::Tmux);
        assert!(matches!(config.transport, TargetTransport::Local));
        // attach 已有 session 不需要工作目录。
        assert!(config.path.is_empty());
    }

    #[test]
    fn empty_socket_is_normalized_away() {
        let mut entry = candidate("demo");
        entry.socket = Some(String::new());
        let row = ExistingRuntime::from_candidate(&entry).unwrap();
        assert_eq!(row.socket, None);
    }

    #[test]
    fn ssh_candidates_keep_the_alias_as_transport_target() {
        let mut entry = candidate("ops");
        entry.transport = "ssh".to_string();
        entry.target = "box".to_string();
        let row = ExistingRuntime::from_candidate(&entry).unwrap();
        let config = row.config();
        assert!(matches!(
            config.transport,
            TargetTransport::Ssh { ref name } if name == "box"
        ));
        assert_eq!(row.detail(), "tmux @ box · 2 windows · attached");
    }

    /// 对照 `ConnectionDiscovery.listExistingConnections` 的 `currentSocket` 分支：
    /// 隔离 socket 上的 session 也要能 attach（否则 `-L` 起的 server 不可达）。
    #[test]
    fn isolated_socket_sessions_carry_the_socket() {
        let entry = TmuxSessionEntry {
            name: "demo".to_string(),
            windows: 1,
            attached: true,
            created: 0,
        };
        let row = ExistingRuntime::from_tmux_session(&entry, Some("muxterm-test-1"));
        assert_eq!(row.session, "demo");
        assert_eq!(row.socket.as_deref(), Some("muxterm-test-1"));
        assert_eq!(row.windows, 1);
        assert!(row.attached);
        assert_eq!(row.config().socket.as_deref(), Some("muxterm-test-1"));
    }

    #[test]
    fn candidates_without_a_session_are_dropped() {
        let mut entry = candidate("");
        entry.session = None;
        assert!(ExistingRuntime::from_candidate(&entry).is_none());
    }
}
