//! WorkspaceSpec：platform 打开工作区时只传产品字段的规格。
//!
//! W10：GUI 打开工作区走 `WorkspacePool::open_spec`，Runtime 构造在 core。
//! CLI 的 `routing.rs` / `daemon.rs` / `tmux_cli_exec.rs` 仍直接构造
//! TmuxRuntime（W12 遗留，未统一）；spec 携带 runtime / transport / name /
//! socket / ssh / dir，core 内部决定用 TmuxRuntime / ShellRuntime / DaemonRuntime。

use crate::core::model::Runtime;
use crate::core::runtime::{DaemonRuntime, HerdrRuntime, HerdrSession, ShellRuntime, TmuxRuntime};
use crate::core::workspace::id::WorkspaceId;
use std::sync::Arc;

/// 打开一个工作区的产品规格（不含 tmux 词）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSpec {
    pub transport: String,
    pub alias: Option<String>,
    pub session: String,
    pub runtime: String,
    pub path: String,
    /// tmux `-L` socket 名（可选；仅 tmux runtime 用）。
    pub socket: Option<String>,
    /// tmux 模式下是否强制 new-session（false = attach 已存在候选）。
    pub create: bool,
    /// attach 初始 capture 的历史行数（W16a：`capture-pane -S -N`）。
    pub scrollback_lines: u32,
}

impl WorkspaceSpec {
    pub fn local_tmux(session: Option<String>, socket: Option<String>) -> Self {
        Self {
            transport: "local".into(),
            alias: None,
            session: session.unwrap_or_default(),
            runtime: "tmux".into(),
            path: String::new(),
            socket,
            create: false,
            scrollback_lines: 10_000,
        }
    }

    /// 本地 tmux new-session 模式。
    pub fn local_tmux_create(session: String, socket: Option<String>) -> Self {
        let mut spec = Self::local_tmux(Some(session), socket);
        spec.create = true;
        spec
    }

    pub fn ssh_tmux(alias: String, session: Option<String>, socket: Option<String>) -> Self {
        Self {
            transport: "ssh".into(),
            alias: Some(alias),
            session: session.unwrap_or_default(),
            runtime: "tmux".into(),
            path: String::new(),
            socket,
            create: false,
            scrollback_lines: 10_000,
        }
    }

    /// Herdr workspace 规格：session = named session 名，path = Herdr workspace_id，
    /// socket = API socket 绝对路径。
    pub fn herdr(
        session_name: impl Into<String>,
        herdr_workspace_id: impl Into<String>,
        socket_path: impl Into<String>,
    ) -> Self {
        Self {
            transport: "local".into(),
            alias: None,
            session: session_name.into(),
            runtime: "herdr".into(),
            path: herdr_workspace_id.into(),
            socket: Some(socket_path.into()),
            create: false,
            scrollback_lines: 10_000,
        }
    }

    pub fn local_shell(path: impl Into<String>) -> Self {
        Self {
            transport: "local".into(),
            alias: None,
            session: String::new(),
            runtime: "shell".into(),
            path: path.into(),
            socket: None,
            create: false,
            scrollback_lines: 10_000,
        }
    }

    /// SSH Herdr：远端 socket 已转发到本机 `socket_path` 后 attach。
    pub fn ssh_herdr(
        alias: impl Into<String>,
        session_name: impl Into<String>,
        herdr_workspace_id: impl Into<String>,
        socket_path: impl Into<String>,
    ) -> Self {
        Self {
            transport: "ssh".into(),
            alias: Some(alias.into()),
            session: session_name.into(),
            runtime: "herdr".into(),
            path: herdr_workspace_id.into(),
            socket: Some(socket_path.into()),
            create: false,
            scrollback_lines: 10_000,
        }
    }

    /// 设置 attach 初始 capture 的历史行数（W16a）。
    pub fn with_scrollback_lines(mut self, lines: u32) -> Self {
        self.scrollback_lines = lines.max(1);
        self
    }

    /// 稳定 WorkspaceId。
    pub fn id(&self) -> WorkspaceId {
        WorkspaceId::new(
            &self.transport,
            self.alias.as_deref(),
            &self.session,
            &self.runtime,
            &self.path,
        )
    }

    /// 用户可见工作区名。
    pub fn name(&self) -> String {
        if self.session.is_empty() {
            crate::core::quickconnect::model::QuickConnect::default_name(&self.path)
        } else {
            self.session.clone()
        }
    }

    /// 构造 Runtime（唯一允许出现 TmuxRuntime 名字的 core 入口之一）。
    pub fn build_runtime(&self) -> Box<dyn Runtime> {
        match self.runtime.as_str() {
            "tmux" if self.transport == "ssh" => {
                let alias = self.alias.as_deref().unwrap_or("");
                let lines = self.scrollback_lines;
                if self.session.is_empty() {
                    let mut rt = TmuxRuntime::new_ssh(alias, self.socket.as_deref());
                    rt.set_scrollback_lines(lines);
                    Box::new(rt)
                } else {
                    let mut rt =
                        TmuxRuntime::new_ssh_attach(alias, self.socket.as_deref(), &self.session);
                    rt.set_scrollback_lines(lines);
                    Box::new(rt)
                }
            }
            "tmux" => {
                let lines = self.scrollback_lines;
                if self.session.is_empty() {
                    let mut rt = TmuxRuntime::new(self.socket.as_deref());
                    rt.set_scrollback_lines(lines);
                    Box::new(rt)
                } else if self.create {
                    let mut rt =
                        TmuxRuntime::new_with_session_name(self.socket.as_deref(), &self.session);
                    rt.set_scrollback_lines(lines);
                    Box::new(rt)
                } else {
                    let mut rt =
                        TmuxRuntime::new_with_attach(self.socket.as_deref(), &self.session);
                    rt.set_scrollback_lines(lines);
                    Box::new(rt)
                }
            }
            "herdr" => {
                let session =
                    HerdrSession::new(&self.session, self.socket.clone().unwrap_or_default());
                Box::new(HerdrRuntime::new(Arc::new(session), &self.path))
            }
            "daemon" => {
                let name = if self.session.is_empty() {
                    "default"
                } else {
                    &self.session
                };
                let path = if self.path.is_empty() {
                    DaemonRuntime::default_socket_path(name)
                } else {
                    std::path::PathBuf::from(&self.path)
                };
                Box::new(DaemonRuntime::new(path, name))
            }
            _ => Box::new(ShellRuntime::new("$SHELL", &self.path)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::runtime::tmux::backend::TmuxRuntime;
    use crate::core::runtime::tmux::client::ConnectMode;

    #[test]
    fn id_and_name_are_stable() {
        let spec = WorkspaceSpec::local_tmux(Some("demo".into()), Some("sock".into()));
        let id = spec.id();
        assert_eq!(id.transport, "local");
        assert_eq!(id.session, "demo");
        assert_eq!(id.runtime, "tmux");
        assert_eq!(spec.name(), "demo");

        let shell = WorkspaceSpec::local_shell("/tmp/work");
        assert_eq!(shell.id().path, "/tmp/work");
        assert_eq!(shell.id().runtime, "shell");
        assert!(!shell.name().is_empty());
    }

    #[test]
    fn local_attach_vs_create_build_different_modes() {
        let attach = WorkspaceSpec::local_tmux(Some("demo".into()), None);
        let create = WorkspaceSpec::local_tmux_create("demo".into(), None);

        let rt_attach = attach.build_runtime();
        let rt_create = create.build_runtime();
        assert_eq!(rt_attach.workspace_runtime(), "tmux");
        assert_eq!(rt_create.workspace_runtime(), "tmux");

        let tmux_attach = rt_attach
            .as_any()
            .downcast_ref::<TmuxRuntime>()
            .expect("attach 应构造 TmuxRuntime");
        let tmux_create = rt_create
            .as_any()
            .downcast_ref::<TmuxRuntime>()
            .expect("create 应构造 TmuxRuntime");
        assert!(matches!(
            tmux_attach.test_connect_mode(),
            Some(ConnectMode::Attach { .. })
        ));
        assert!(matches!(
            tmux_create.test_connect_mode(),
            Some(ConnectMode::NewSession { .. })
        ));
    }

    #[test]
    fn ssh_empty_session_builds_ssh_runtime() {
        let spec = WorkspaceSpec::ssh_tmux("myhost".into(), None, None);
        let rt = spec.build_runtime();
        assert_eq!(rt.workspace_runtime(), "tmux");
        let tmux = rt
            .as_any()
            .downcast_ref::<TmuxRuntime>()
            .expect("ssh 应构造 TmuxRuntime");
        // 空 session → new_ssh（无 attach 模式）。
        assert!(tmux.test_connect_mode().is_none());
    }

    #[test]
    fn unknown_runtime_builds_shell() {
        let spec = WorkspaceSpec {
            transport: "local".into(),
            alias: None,
            session: String::new(),
            runtime: "unknown".into(),
            path: "/tmp/x".into(),
            socket: None,
            create: false,
            scrollback_lines: 10_000,
        };
        let rt = spec.build_runtime();
        assert_eq!(rt.workspace_runtime(), "shell");
    }
}
