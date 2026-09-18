//! HerdrDriver：包装 HerdrRuntime + HerdrSession；SSH 走 socket 转发。

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;

use crate::protocol::candidate::ExistingCandidate;
use crate::runtime::herdr::runtime::HerdrRuntime;
use crate::runtime::herdr::session::HerdrSession;
use crate::runtime::RuntimeProvider;
use crate::runtime::{
    Runtime, RuntimeCapability, RuntimeError, RuntimeNamespace, RuntimeResult, RuntimeSpec,
};
use crate::transport::{ChannelKind, Connect, TargetConnection};

/// herdr 插件（local / ssh）。
pub struct HerdrDriver;

/// 本地 Herdr API socket：default → `~/.config/herdr/herdr.sock`；
/// named → `~/.config/herdr/sessions/<name>/herdr.sock`。
/// `HERDR_SOCKET_PATH` 仍可整体覆盖（测试隔离用）。
fn local_herdr_socket_for(session: &str) -> String {
    if let Ok(path) = std::env::var("HERDR_SOCKET_PATH") {
        let path = path.trim();
        if !path.is_empty() {
            return path.to_string();
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let base = std::path::PathBuf::from(home).join(".config/herdr");
    if session.is_empty() || session == "default" {
        base.join("herdr.sock").to_string_lossy().into_owned()
    } else {
        base.join("sessions")
            .join(session)
            .join("herdr.sock")
            .to_string_lossy()
            .into_owned()
    }
}

impl RuntimeProvider for HerdrDriver {
    fn id(&self) -> &'static str {
        "herdr"
    }

    fn name(&self) -> &'static str {
        "herdr"
    }

    fn support(&self) -> &'static [RuntimeCapability] {
        &[
            RuntimeCapability::PersistDetach,
            RuntimeCapability::Discover,
            RuntimeCapability::MultiTab,
            RuntimeCapability::SplitPane,
            RuntimeCapability::WorktreeList,
            RuntimeCapability::WorktreeCreate,
            RuntimeCapability::WorktreeOpen,
        ]
    }

    fn channel_requirements(&self) -> &'static [ChannelKind] {
        &[ChannelKind::UnixSocket]
    }

    fn discover(
        &self,
        connect: &dyn TargetConnection,
        namespace: Option<&str>,
    ) -> crate::runtime::RuntimeResult<Vec<ExistingCandidate>> {
        if connect.transport_id() == "ssh" {
            let entries = crate::discovery::existing::discover_ssh_herdr(
                connect.target(),
                std::env::var("MUXTERM_SSH_CONFIG_PATH").ok().as_deref(),
                Duration::from_secs(2),
            );
            return Ok(entries
                .into_iter()
                .filter(|e| {
                    namespace.is_none_or(|ns| ns == e.herdr_session.as_deref().unwrap_or(""))
                })
                .map(|e| ExistingCandidate {
                    runtime_id: "herdr".into(),
                    transport_id: connect.transport_id().into(),
                    target: connect.target().into(),
                    namespace: e.herdr_session.clone(),
                    name: e.title,
                    extra: e.herdr_workspace_id.clone().unwrap_or_default(),
                    // W6 §11.1：typed 身份字段由 Core 转换。
                    session: e.herdr_session.clone(),
                    socket: e.herdr_socket.clone(),
                    workspace_id: e.herdr_workspace_id.clone(),
                })
                .collect());
        }
        // 本地：HERDR_SOCKET_PATH / config_dir 注入由调用方负责；无注入也允许
        // 扫本机（W20 生产行为）。
        let entries = crate::discovery::existing::discover_local_herdr(None);
        Ok(entries
            .into_iter()
            .filter(|e| namespace.is_none_or(|ns| ns == e.herdr_session.as_deref().unwrap_or("")))
            .map(|e| ExistingCandidate {
                runtime_id: "herdr".into(),
                transport_id: "local".into(),
                target: String::new(),
                namespace: e.herdr_session.clone(),
                name: e.title,
                extra: e.herdr_workspace_id.clone().unwrap_or_default(),
                // W6 §11.1：typed 身份字段由 Core 转换。
                session: e.herdr_session.clone(),
                socket: e.herdr_socket.clone(),
                workspace_id: e.herdr_workspace_id.clone(),
            })
            .collect())
    }

    fn namespaces(
        &self,
        connect: &dyn TargetConnection,
    ) -> crate::runtime::RuntimeResult<Vec<RuntimeNamespace>> {
        if connect.transport_id() == "ssh" {
            let sessions = crate::discovery::existing::ssh_herdr_sessions(
                connect.target(),
                std::env::var("MUXTERM_SSH_CONFIG_PATH").ok().as_deref(),
                Duration::from_secs(2),
            )
            .unwrap_or_default();
            return Ok(sessions
                .into_iter()
                .map(|(name, socket)| RuntimeNamespace {
                    name: if name.trim().is_empty() {
                        "default".into()
                    } else {
                        name
                    },
                    socket: (!socket.trim().is_empty()).then_some(socket),
                })
                .collect());
        }
        let config_dir = std::env::var("HERDR_CONFIG_DIR")
            .ok()
            .map(std::path::PathBuf::from);
        Ok(
            crate::discovery::existing::discover_local_herdr_namespaces(config_dir.as_deref())
                .into_iter()
                .map(|(name, socket)| RuntimeNamespace {
                    name,
                    socket: Some(socket),
                })
                .collect(),
        )
    }

    fn new_instance(
        &self,
        connect: Arc<dyn TargetConnection>,
        spec: &RuntimeSpec,
    ) -> crate::runtime::RuntimeResult<Box<dyn Runtime>> {
        let session_name = if spec.session.is_empty() {
            "default"
        } else {
            &spec.session
        };
        let socket = match spec.socket.clone() {
            Some(socket) => socket,
            None if connect.transport_id() == "local" => {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{home}/.config/herdr/herdr.sock")
            }
            None => return Err(anyhow!("SSH Herdr 缺远端 socket 路径").into()),
        };
        let session = HerdrSession::shared_with_connection(connect, session_name, socket);
        Ok(Box::new(HerdrRuntime::new(session, &spec.path)))
    }

    fn create_identity(
        &self,
        connection: &dyn TargetConnection,
        spec: &RuntimeSpec,
        label: Option<&str>,
    ) -> RuntimeResult<RuntimeSpec> {
        let is_ssh = connection.transport_id() == "ssh";
        let mut spec = spec.clone();
        if spec.session.is_empty() {
            return Err(RuntimeError::message(
                "Herdr session 未选择。请先选择一个正在运行的 named/default session",
            ));
        }
        if spec.socket.is_none() {
            if is_ssh {
                let alias = connection.target();
                if alias.is_empty() {
                    return Err(RuntimeError::message("SSH Herdr 缺 target alias"));
                }
                let socket = crate::discovery::existing::ssh_herdr_running_socket(
                    alias,
                    &spec.session,
                    std::env::var("MUXTERM_SSH_CONFIG_PATH").ok().as_deref(),
                    Duration::from_secs(2),
                )
                .ok_or_else(|| {
                    RuntimeError::message(format!(
                        "SSH Herdr session `{}` 未运行（{}）。先在远端启动 herdr，或从已有连接里选一个 session",
                        spec.session, alias
                    ))
                })?;
                spec.socket = Some(socket);
            } else {
                spec.socket = Some(local_herdr_socket_for(&spec.session));
            }
        }
        let socket = spec.socket.as_deref().expect("herdr socket 已补齐");
        let connection = Connect::new(connection.transport_id(), connection.target());
        let herdr = HerdrSession::with_connection(connection, &spec.session, socket);
        if herdr.ping().is_err() {
            return Err(RuntimeError::message(format!(
                "Herdr session `{}` 未运行（{}）。先启动 herdr，或从已有连接里选一个 session",
                spec.session, socket
            )));
        }
        // SSH cwd 保持远端字面量（含 ~）；本地才展开本机 HOME。
        let cwd = if is_ssh {
            spec.path.clone()
        } else {
            crate::executable::expand_config_value(&spec.path)
        };
        let created = herdr
            .workspace_create(&cwd, label.unwrap_or(&spec.session))
            .map_err(RuntimeError::message)?;
        spec.path = created.workspace_id;
        Ok(spec)
    }
}
