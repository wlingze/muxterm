//! Runtime 层：建立在 Transport 之上，理解终端语义。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §4。
//!
//! 扩展规则：新增 Runtime 不修改 Transport、不修改 Core Protocol。
//! - `shell`：ShellRuntime（自管 pane 分割 + shell 进程生命周期）
//! - `tmux`：TmuxRuntime（tmux 控制模式；内部含 adapter）
//! - `daemon`：DaemonBackend（IPC client，连本地 daemon）
//!
//! Runtime 不关心 Transport 是 local 还是 SSH；Transport 不理解 shell/tmux 语义。
//! tmux 的 %pane/@window 等真实 ID 只能在 runtime/tmux 内部。

pub mod daemon;
pub mod shell;
pub mod tmux;

// Re-export Backend trait from model
pub use crate::core::model::backend::Backend;

// Re-export backend implementations
pub use daemon::DaemonBackend;
pub use shell::LocalBackend;
pub use tmux::backend::TmuxBackend;

/// 运行时模式：四种组合的入口。
///
/// 2 个 Transport × 2 个 Runtime = 4 种组合。
/// `create_backend` 工厂根据模式构造对应的 Backend 实例。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeMode {
    /// local-shell = LocalProcessTransport + ShellRuntime
    LocalShell { session_name: Option<String> },
    /// local-tmux = LocalProcessTransport + TmuxRuntime
    LocalTmux {
        socket: Option<String>,
        session_name: Option<String>,
    },
    /// ssh-shell = SshProcessTransport + ShellRuntime
    SshShell {
        alias: String,
        session_name: Option<String>,
    },
    /// ssh-tmux = SshProcessTransport + TmuxRuntime
    SshTmux {
        alias: String,
        session_name: Option<String>,
    },
}

impl RuntimeMode {
    /// 是否为 tmux 模式。
    pub fn is_tmux(&self) -> bool {
        matches!(
            self,
            RuntimeMode::LocalTmux { .. } | RuntimeMode::SshTmux { .. }
        )
    }

    /// 是否为 SSH 模式。
    pub fn is_ssh(&self) -> bool {
        matches!(
            self,
            RuntimeMode::SshShell { .. } | RuntimeMode::SshTmux { .. }
        )
    }

    /// 获取 session name（如果有）。
    pub fn session_name(&self) -> Option<&str> {
        match self {
            RuntimeMode::LocalShell { session_name }
            | RuntimeMode::LocalTmux { session_name, .. }
            | RuntimeMode::SshShell { session_name, .. }
            | RuntimeMode::SshTmux { session_name, .. } => session_name.as_deref(),
        }
    }

    /// 获取 SSH alias（如果有）。
    pub fn ssh_alias(&self) -> Option<&str> {
        match self {
            RuntimeMode::SshShell { alias, .. } | RuntimeMode::SshTmux { alias, .. } => Some(alias),
            _ => None,
        }
    }

    /// 获取 tmux socket（如果有）。
    pub fn tmux_socket(&self) -> Option<&str> {
        match self {
            RuntimeMode::LocalTmux { socket, .. } => socket.as_deref(),
            _ => None,
        }
    }
}

/// 根据 RuntimeMode 构造对应的 Backend 实例。
///
/// **注意**：当前实现为兼容 facade，内部委托给现有 `LocalBackend` / `TmuxBackend`
/// 构造逻辑。后续阶段逐步替换为 ShellRuntime / TmuxRuntime + Transport。
pub fn create_backend(mode: &RuntimeMode) -> Box<dyn Backend> {
    match mode {
        RuntimeMode::LocalShell { .. } => Box::new(LocalBackend::new("$SHELL", "")),
        RuntimeMode::LocalTmux {
            socket,
            session_name,
        } => {
            if let Some(name) = session_name {
                Box::new(TmuxBackend::new_with_session_name(socket.as_deref(), name))
            } else {
                Box::new(TmuxBackend::new(socket.as_deref()))
            }
        }
        RuntimeMode::SshShell { .. } => {
            // v1: SSH shell 尚未完全接入，fallback 到 LocalBackend
            // TODO(phase 4): SshProcessTransport + ShellRuntime
            Box::new(LocalBackend::new("$SHELL", ""))
        }
        RuntimeMode::SshTmux { .. } => {
            // v1: SSH tmux 尚未完全接入
            // TODO(phase 4): SshProcessTransport + TmuxRuntime
            Box::new(LocalBackend::new("$SHELL", ""))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_mode_is_tmux() {
        assert!(!RuntimeMode::LocalShell { session_name: None }.is_tmux());
        assert!(RuntimeMode::LocalTmux {
            socket: None,
            session_name: None
        }
        .is_tmux());
        assert!(!RuntimeMode::SshShell {
            alias: "host".into(),
            session_name: None
        }
        .is_tmux());
        assert!(RuntimeMode::SshTmux {
            alias: "host".into(),
            session_name: None
        }
        .is_tmux());
    }

    #[test]
    fn runtime_mode_is_ssh() {
        assert!(!RuntimeMode::LocalShell { session_name: None }.is_ssh());
        assert!(RuntimeMode::SshShell {
            alias: "host".into(),
            session_name: None
        }
        .is_ssh());
        assert!(RuntimeMode::SshTmux {
            alias: "host".into(),
            session_name: None
        }
        .is_ssh());
    }

    #[test]
    fn runtime_mode_session_name() {
        assert_eq!(
            RuntimeMode::LocalShell {
                session_name: Some("dev".into())
            }
            .session_name(),
            Some("dev")
        );
        assert_eq!(
            RuntimeMode::LocalTmux {
                socket: None,
                session_name: Some("test".into())
            }
            .session_name(),
            Some("test")
        );
        assert_eq!(
            RuntimeMode::SshShell {
                alias: "host".into(),
                session_name: None
            }
            .session_name(),
            None
        );
    }

    #[test]
    fn runtime_mode_ssh_alias() {
        assert_eq!(
            RuntimeMode::SshShell {
                alias: "server".into(),
                session_name: None
            }
            .ssh_alias(),
            Some("server")
        );
        assert_eq!(
            RuntimeMode::LocalShell { session_name: None }.ssh_alias(),
            None
        );
    }

    #[test]
    fn runtime_mode_tmux_socket() {
        assert_eq!(
            RuntimeMode::LocalTmux {
                socket: Some("mysock".into()),
                session_name: None
            }
            .tmux_socket(),
            Some("mysock")
        );
        assert_eq!(
            RuntimeMode::LocalShell { session_name: None }.tmux_socket(),
            None
        );
    }

    #[test]
    fn create_backend_local_shell() {
        let mode = RuntimeMode::LocalShell { session_name: None };
        let backend = create_backend(&mode);
        // 验证 Backend 可构造（不 panic）
        let _ = backend.status();
    }

    #[test]
    fn create_backend_local_tmux() {
        let mode = RuntimeMode::LocalTmux {
            socket: None,
            session_name: Some("test-runtime-create".into()),
        };
        let backend = create_backend(&mode);
        // 不 connect，只验证可构造
        let _ = backend.status();
    }

    /// 验证 RuntimeMode 四种组合可区分。
    #[test]
    fn runtime_mode_four_combinations_distinct() {
        let modes = vec![
            RuntimeMode::LocalShell { session_name: None },
            RuntimeMode::LocalTmux {
                socket: None,
                session_name: None,
            },
            RuntimeMode::SshShell {
                alias: "a".into(),
                session_name: None,
            },
            RuntimeMode::SshTmux {
                alias: "a".into(),
                session_name: None,
            },
        ];
        assert_eq!(modes.len(), 4);
        assert!(modes[0] != modes[1]);
        assert!(modes[1] != modes[2]);
        assert!(modes[2] != modes[3]);
        assert!(modes[0] != modes[3]);
    }
}
