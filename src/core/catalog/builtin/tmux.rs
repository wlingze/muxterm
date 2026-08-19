//! TmuxDriver：包装 TmuxRuntime + 现有 tmux 发现。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::core::catalog::connect::Connect;
use crate::core::catalog::driver::{RuntimeDriver, SessionCandidate};
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::runtime::tmux::backend::TmuxRuntime;
use crate::core::workspace::spec::WorkspaceSpec;

/// tmux 插件（local / ssh）。
pub struct TmuxDriver;

impl TmuxDriver {
    fn ssh_config() -> Option<String> {
        std::env::var("MUXTERM_SSH_CONFIG_PATH").ok()
    }
}

impl RuntimeDriver for TmuxDriver {
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

    fn accepted_transports(&self) -> &'static [&'static str] {
        &["local", "ssh"]
    }

    fn list(&self, connect: &Connect, _namespace: Option<&str>) -> Result<Vec<SessionCandidate>> {
        let ssh_config = Self::ssh_config();
        let sessions = if connect.transport_id() == "ssh" {
            // 测试隔离远端 tmux：MUXTERM_TEST_REMOTE_TMUX_SOCKET（对标
            // HERDR_SOCKET_PATH）。生产不设 = 远端默认 server。
            let remote_socket = std::env::var("MUXTERM_TEST_REMOTE_TMUX_SOCKET").ok();
            crate::core::discovery::list_ssh_tmux_sessions(
                connect.target(),
                ssh_config.as_deref(),
                remote_socket.as_deref(),
                Duration::from_secs(2),
            )
            .unwrap_or_default()
        } else {
            // 测试隔离本地 tmux：MUXTERM_TEST_LOCAL_TMUX_SOCKET（对标 REMOTE env）。
            let local_socket = std::env::var("MUXTERM_TEST_LOCAL_TMUX_SOCKET").ok();
            crate::core::discovery::list_local_tmux_sessions(local_socket.as_deref())
        };
        Ok(sessions
            .into_iter()
            .map(|s| SessionCandidate {
                runtime_id: "tmux".into(),
                transport_id: connect.transport_id().into(),
                target: connect.target().into(),
                namespace: None,
                name: s.name,
                extra: String::new(),
            })
            .collect())
    }

    fn open(&self, connect: Arc<Connect>, spec: &WorkspaceSpec) -> Result<Box<dyn Runtime>> {
        let mut rt = if connect.transport_id() == "ssh" {
            if spec.session.is_empty() {
                TmuxRuntime::new_ssh(connect.target(), spec.socket.as_deref())
            } else {
                TmuxRuntime::new_ssh_attach(connect.target(), spec.socket.as_deref(), &spec.session)
            }
        } else if spec.session.is_empty() {
            TmuxRuntime::new(spec.socket.as_deref())
        } else if spec.create {
            TmuxRuntime::new_with_session_name(spec.socket.as_deref(), &spec.session)
        } else {
            TmuxRuntime::new_with_attach(spec.socket.as_deref(), &spec.session)
        };
        rt.set_scrollback_lines(spec.scrollback_lines);
        Ok(Box::new(rt))
    }
}
