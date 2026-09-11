//! TmuxDriver：包装 TmuxRuntime + 现有 tmux 发现。

use std::sync::Arc;
use std::time::Duration;

use crate::protocol::candidate::ExistingCandidate;
use crate::runtime::tmux::backend::TmuxRuntime;
use crate::runtime::tmux::status::{fetch_snapshot, StatusQueryConfig};
use crate::runtime::RuntimeProvider;
use crate::runtime::RuntimeSpec;
use crate::runtime::{Runtime, RuntimeCapability, RuntimeError, RuntimeResult};
use crate::transport::TargetConnection;

/// tmux 插件（local / ssh）。
pub struct TmuxDriver;

impl TmuxDriver {
    fn ssh_config() -> Option<String> {
        std::env::var("MUXTERM_SSH_CONFIG_PATH").ok()
    }
}

fn sanitize_session_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        "worktree".into()
    } else {
        out
    }
}

impl RuntimeProvider for TmuxDriver {
    fn id(&self) -> &'static str {
        "tmux"
    }

    fn name(&self) -> &'static str {
        "tmux"
    }

    fn support(&self) -> &'static [RuntimeCapability] {
        &[
            RuntimeCapability::PersistDetach,
            RuntimeCapability::Discover,
            RuntimeCapability::MultiTab,
            RuntimeCapability::SplitPane,
            RuntimeCapability::SharedClientResize,
        ]
    }

    fn discover(
        &self,
        connect: &dyn TargetConnection,
        _namespace: Option<&str>,
    ) -> crate::runtime::RuntimeResult<Vec<ExistingCandidate>> {
        let ssh_config = Self::ssh_config();
        let (sessions, socket) = if connect.transport_id() == "ssh" {
            // 测试隔离远端 tmux：MUXTERM_TEST_REMOTE_TMUX_SOCKET（对标
            // HERDR_SOCKET_PATH）。生产不设 = 远端默认 server。
            let remote_socket = std::env::var("MUXTERM_TEST_REMOTE_TMUX_SOCKET").ok();
            let sessions = crate::discovery::list_ssh_tmux_sessions(
                connect.target(),
                ssh_config.as_deref(),
                remote_socket.as_deref(),
                Duration::from_secs(2),
            )
            .unwrap_or_default();
            (sessions, remote_socket)
        } else {
            // 测试隔离本地 tmux：MUXTERM_TEST_LOCAL_TMUX_SOCKET（对标 REMOTE env）。
            let local_socket = std::env::var("MUXTERM_TEST_LOCAL_TMUX_SOCKET").ok();
            let sessions = crate::discovery::list_local_tmux_sessions(local_socket.as_deref());
            (sessions, local_socket)
        };
        Ok(sessions
            .into_iter()
            .map(|s| ExistingCandidate {
                runtime_id: "tmux".into(),
                transport_id: connect.transport_id().into(),
                target: connect.target().into(),
                namespace: None,
                name: s.name.clone(),
                extra: String::new(),
                // W6 §11.1：Existing attach 的 tmux session/socket 也必须
                // 保留 typed 身份；否则测试 socket 会退回默认 server。
                session: Some(s.name),
                socket: socket.clone(),
                workspace_id: None,
            })
            .collect())
    }

    fn new_instance(
        &self,
        connect: Arc<dyn TargetConnection>,
        spec: &RuntimeSpec,
    ) -> crate::runtime::RuntimeResult<Box<dyn Runtime>> {
        let mut rt = TmuxRuntime::new_with_connection_and_cwd(
            connect,
            spec.socket.as_deref(),
            (!spec.session.is_empty()).then_some(spec.session.as_str()),
            spec.create,
            (!spec.path.is_empty()).then_some(spec.path.as_str()),
        );
        rt.set_scrollback_lines(spec.scrollback_lines);
        Ok(Box::new(rt))
    }

    fn status_snapshot(
        &self,
        connection: &dyn TargetConnection,
        session: &str,
        socket: Option<&str>,
    ) -> RuntimeResult<serde_json::Value> {
        let ssh_alias = if connection.transport_id() == "ssh" {
            Some(connection.target().to_string())
        } else {
            None
        };
        let cfg = StatusQueryConfig {
            socket: socket.map(ToOwned::to_owned),
            ssh_alias,
            session: session.to_string(),
        };
        let status = fetch_snapshot(&cfg).map_err(RuntimeError::message)?;
        serde_json::to_value(status).map_err(RuntimeError::message)
    }

    fn worktree_session_name(&self, project_id: &str, worktree_id: &str) -> Option<String> {
        Some(format!(
            "{}/{}",
            sanitize_session_component(project_id),
            sanitize_session_component(worktree_id)
        ))
    }
}
