//! TmuxDriver：包装 TmuxRuntime + 现有 tmux 发现。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::protocol::candidate::ExistingCandidate;
use crate::runtime::tmux::backend::TmuxRuntime;
use crate::runtime::RuntimeProvider;
use crate::runtime::{Runtime, RuntimeCapability};
use crate::transport::TargetConnection;
use muxterm_runtime::RuntimeSpec;

/// tmux 插件（local / ssh）。
pub struct TmuxDriver;

impl TmuxDriver {
    fn ssh_config() -> Option<String> {
        std::env::var("MUXTERM_SSH_CONFIG_PATH").ok()
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
    ) -> Result<Vec<ExistingCandidate>> {
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
    ) -> Result<Box<dyn Runtime>> {
        let mut rt = TmuxRuntime::new_with_connection(
            connect,
            spec.socket.as_deref(),
            (!spec.session.is_empty()).then_some(spec.session.as_str()),
            spec.create,
        );
        rt.set_scrollback_lines(spec.scrollback_lines);
        Ok(Box::new(rt))
    }
}
