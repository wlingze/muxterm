//! WorkspaceSpec：platform 打开工作区时只传产品字段的规格。
//!
//! W10：GUI 打开工作区走 `WorkspacePool::open_spec`，Runtime 构造在 core。
//! CLI 的 `routing.rs` / `daemon.rs` / `tmux_cli_exec.rs` 仍直接构造
//! Runtime（W12 遗留，未统一）；spec 只携带 runtime / transport / name /
//! socket / ssh / dir 等解析结果字段。

use crate::workspace::provenance::WorkspaceProvenance;
use crate::workspace::template::TemplateName;
use muxterm_protocol::WorkspaceId;

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
    /// 从 Projects 打开时携带的 Project/Worktree 归属。
    pub provenance: Option<WorkspaceProvenance>,
    /// 仅在新建 Workspace 时应用的模板名称；attach 不应执行模板。
    pub template: Option<TemplateName>,
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
            provenance: None,
            template: None,
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
            provenance: None,
            template: None,
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
            provenance: None,
            template: None,
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
            provenance: None,
            template: None,
        }
    }

    pub fn ssh_shell(alias: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            transport: "ssh".into(),
            alias: Some(alias.into()),
            session: String::new(),
            runtime: "shell".into(),
            path: path.into(),
            socket: None,
            create: false,
            scrollback_lines: 10_000,
            provenance: None,
            template: None,
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
            provenance: None,
            template: None,
        }
    }

    /// 设置 attach 初始 capture 的历史行数（W16a）。
    pub fn with_scrollback_lines(mut self, lines: u32) -> Self {
        self.scrollback_lines = lines.max(1);
        self
    }

    /// Convert the Core-owned product spec to the runtime crate boundary.
    pub fn runtime_spec(&self) -> muxterm_runtime::RuntimeSpec {
        muxterm_runtime::RuntimeSpec {
            transport: self.transport.clone(),
            alias: self.alias.clone(),
            session: self.session.clone(),
            runtime: self.runtime.clone(),
            path: self.path.clone(),
            socket: self.socket.clone(),
            create: self.create,
            scrollback_lines: self.scrollback_lines,
        }
    }

    /// Rebuild a product spec returned by a Runtime-native operation.
    pub fn from_runtime_spec(spec: muxterm_runtime::RuntimeSpec) -> Self {
        Self {
            transport: spec.transport,
            alias: spec.alias,
            session: spec.session,
            runtime: spec.runtime,
            path: spec.path,
            socket: spec.socket,
            create: spec.create,
            scrollback_lines: spec.scrollback_lines,
            provenance: None,
            template: None,
        }
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
            crate::quickconnect::model::QuickConnect::default_name(&self.path)
        } else {
            self.session.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::runtime::shell::ShellRuntime;
    use crate::runtime::tmux::backend::TmuxRuntime;
    use crate::runtime::tmux::client::ConnectMode;
    use crate::transport::registry::ConnectionRegistry;

    fn new_runtime(spec: &WorkspaceSpec) -> Box<dyn crate::runtime::Runtime> {
        let mut connections = ConnectionRegistry::new();
        Catalog::with_builtins()
            .new_runtime(&mut connections, spec)
            .expect("built-in provider must construct the runtime")
    }

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

        let ssh_shell = WorkspaceSpec::ssh_shell("dev", "/srv/project");
        assert_eq!(ssh_shell.id().transport, "ssh");
        assert_eq!(ssh_shell.id().alias.as_deref(), Some("dev"));
        assert_eq!(ssh_shell.id().runtime, "shell");
        assert_eq!(ssh_shell.id().path, "/srv/project");
    }

    #[test]
    fn runtime_spec_round_trip_preserves_runtime_fields() {
        let spec = WorkspaceSpec::ssh_tmux("dev".into(), Some("demo".into()), Some("sock".into()))
            .with_scrollback_lines(512);
        let runtime_spec = spec.runtime_spec();
        let rebuilt = WorkspaceSpec::from_runtime_spec(runtime_spec);
        assert_eq!(rebuilt.transport, spec.transport);
        assert_eq!(rebuilt.alias, spec.alias);
        assert_eq!(rebuilt.session, spec.session);
        assert_eq!(rebuilt.runtime, spec.runtime);
        assert_eq!(rebuilt.path, spec.path);
        assert_eq!(rebuilt.socket, spec.socket);
        assert_eq!(rebuilt.create, spec.create);
        assert_eq!(rebuilt.scrollback_lines, spec.scrollback_lines);
        assert!(rebuilt.provenance.is_none());
        assert!(rebuilt.template.is_none());
    }

    #[test]
    fn ssh_shell_builds_shell_runtime_without_local_fallback() {
        let spec = WorkspaceSpec::ssh_shell("dev", "/srv/project");
        let runtime = new_runtime(&spec);
        assert_eq!(runtime.workspace_runtime(), "shell");
        let shell = runtime
            .as_any()
            .downcast_ref::<ShellRuntime>()
            .expect("SSH shell 必须构造 ShellRuntime");
        assert_eq!(shell.test_ssh_alias(), Some("dev"));
    }

    #[test]
    fn local_attach_vs_create_build_different_modes() {
        let attach = WorkspaceSpec::local_tmux(Some("demo".into()), None);
        let create = WorkspaceSpec::local_tmux_create("demo".into(), None);

        let rt_attach = new_runtime(&attach);
        let rt_create = new_runtime(&create);
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
        let rt = new_runtime(&spec);
        assert_eq!(rt.workspace_runtime(), "tmux");
        let tmux = rt
            .as_any()
            .downcast_ref::<TmuxRuntime>()
            .expect("ssh 应构造 TmuxRuntime");
        // 空 session → new_ssh（无 attach 模式）。
        assert!(tmux.test_connect_mode().is_none());
    }

    #[test]
    fn unknown_runtime_is_rejected() {
        let spec = WorkspaceSpec {
            transport: "local".into(),
            alias: None,
            session: String::new(),
            runtime: "unknown".into(),
            path: "/tmp/x".into(),
            socket: None,
            create: false,
            scrollback_lines: 10_000,
            provenance: None,
            template: None,
        };
        let mut connections = ConnectionRegistry::new();
        let err = Catalog::with_builtins()
            .new_runtime(&mut connections, &spec)
            .err();
        assert!(
            err.as_ref()
                .is_some_and(|error| error.to_string().contains("unknown runtime")),
            "unknown runtime must not silently fall back to shell: {err:?}"
        );
    }
}
