//! WorkspaceSpec：platform 打开工作区时只传产品字段的规格。
//!
//! W10：GUI/CLI 不再直接 `new TmuxRuntime`；Runtime 构造只发生在 core。
//! spec 携带 runtime / transport / name / socket / ssh / dir，core 内部
//! 决定用 TmuxRuntime / ShellRuntime / DaemonRuntime。

use crate::core::model::Runtime;
use crate::core::runtime::{DaemonRuntime, ShellRuntime, TmuxRuntime};
use crate::core::workspace::id::WorkspaceId;

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
                if self.session.is_empty() {
                    Box::new(TmuxRuntime::new_ssh(alias, self.socket.as_deref()))
                } else {
                    Box::new(TmuxRuntime::new_ssh_attach(
                        alias,
                        self.socket.as_deref(),
                        &self.session,
                    ))
                }
            }
            "tmux" => {
                if self.session.is_empty() {
                    Box::new(TmuxRuntime::new(self.socket.as_deref()))
                } else if self.create {
                    Box::new(TmuxRuntime::new_with_session_name(
                        self.socket.as_deref(),
                        &self.session,
                    ))
                } else {
                    Box::new(TmuxRuntime::new_with_attach(
                        self.socket.as_deref(),
                        &self.session,
                    ))
                }
            }
            "daemon" => {
                let name = if self.session.is_empty() {
                    "default"
                } else {
                    &self.session
                };
                let path = if self.path.is_empty() {
                    crate::platform::cli::session::session_socket_path(name)
                } else {
                    std::path::PathBuf::from(&self.path)
                };
                Box::new(DaemonRuntime::new(path, name))
            }
            _ => Box::new(ShellRuntime::new("$SHELL", &self.path)),
        }
    }
}
