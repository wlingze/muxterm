//! HerdrDriver：包装 HerdrRuntime + HerdrSession；SSH 走 socket 转发。

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::anyhow;

use crate::protocol::candidate::ExistingCandidate;
use crate::runtime::herdr::runtime::HerdrRuntime;
use crate::runtime::herdr::session::HerdrSession;
use crate::runtime::RuntimeProvider;
use crate::runtime::{
    Runtime, RuntimeCapability, RuntimeError, RuntimeNamespace, RuntimeResult, RuntimeSpec,
};
use crate::transport::{ChannelKind, ChannelRequest, Connect, TargetConnection};

const HERDR_STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const HERDR_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(1);

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
    let base = crate::discovery::existing::local_herdr_config_dir();
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

fn ensure_ssh_herdr_session(
    connection: &dyn TargetConnection,
    session: &str,
    socket_hint: Option<&str>,
) -> RuntimeResult<String> {
    let alias = connection.target();
    if alias.is_empty() {
        return Err(RuntimeError::message("SSH Herdr 缺 target alias"));
    }

    let ssh_config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
    let remote_sessions = crate::discovery::existing::ssh_herdr_sessions(
        alias,
        ssh_config.as_deref(),
        HERDR_DISCOVERY_TIMEOUT,
    );
    if let Some(sessions) = remote_sessions.as_ref() {
        if let Some((_, socket)) = sessions.iter().find(|(name, _)| {
            let name = if name.is_empty() {
                "default"
            } else {
                name.as_str()
            };
            name == session
        }) {
            if !socket.is_empty() {
                if let Some(socket_hint) = socket_hint.filter(|hint| *hint != socket) {
                    let hinted = HerdrSession::with_connection(
                        Connect::new("ssh", alias),
                        session,
                        socket_hint,
                    );
                    if hinted.ping().is_ok() {
                        return Ok(socket_hint.to_string());
                    }
                }
                return Ok(socket.clone());
            }
            if let Some(socket_hint) = socket_hint {
                let hinted =
                    HerdrSession::with_connection(Connect::new("ssh", alias), session, socket_hint);
                if hinted.ping().is_ok() {
                    return Ok(socket_hint.to_string());
                }
            }
            return Err(RuntimeError::message(format!(
                "SSH Herdr session `{session}` 正在运行，但没有返回 socket 路径"
            )));
        }
    }

    // A configured socket may still be live while session discovery is
    // temporarily unavailable. Probe it before starting another server.
    if let Some(socket) = socket_hint {
        let existing = HerdrSession::with_connection(Connect::new("ssh", alias), session, socket);
        if existing.ping().is_ok() {
            return Ok(socket.to_string());
        }
    }

    start_ssh_herdr_server(connection, session)?;
    let deadline = Instant::now() + HERDR_STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(sessions) = crate::discovery::existing::ssh_herdr_sessions(
            alias,
            ssh_config.as_deref(),
            HERDR_DISCOVERY_TIMEOUT,
        ) {
            if let Some((_, socket)) = sessions.iter().find(|(name, _)| {
                let name = if name.is_empty() {
                    "default"
                } else {
                    name.as_str()
                };
                name == session
            }) {
                if !socket.is_empty() {
                    if let Some(socket_hint) = socket_hint.filter(|hint| *hint != socket) {
                        let hinted = HerdrSession::with_connection(
                            Connect::new("ssh", alias),
                            session,
                            socket_hint,
                        );
                        if hinted.ping().is_ok() {
                            return Ok(socket_hint.to_string());
                        }
                    }
                    return Ok(socket.clone());
                }
            }
        }
        if let Some(socket) = socket_hint {
            let session_api =
                HerdrSession::with_connection(Connect::new("ssh", alias), session, socket);
            if session_api.ping().is_ok() {
                return Ok(socket.to_string());
            }
        }
        thread::sleep(Duration::from_millis(150));
    }

    Err(RuntimeError::message(format!(
        "已在 SSH target `{alias}` 启动 Herdr session `{session}`，但 {HERDR_STARTUP_TIMEOUT:?} 内没有发现可用 socket"
    )))
}

fn start_ssh_herdr_server(connection: &dyn TargetConnection, session: &str) -> RuntimeResult<()> {
    const START_SCRIPT: &str = "unset HERDR_ENV HERDR_SESSION; PATH=\"$HOME/.local/bin:$PATH\"; export PATH; command -v herdr >/dev/null 2>&1 || { printf '%s\\n' 'herdr executable not found in PATH' >&2; exit 127; }; nohup herdr --session \"$1\" server >/dev/null 2>&1 </dev/null &";
    let output = connection
        .exec_command(ChannelRequest::Exec {
            argv: vec![
                "sh".into(),
                "-c".into(),
                START_SCRIPT.into(),
                "muxterm".into(),
                session.into(),
            ],
            cwd: None,
            env: Vec::new(),
            pty: None,
        })
        .map_err(|error| {
            RuntimeError::message(format!(
                "通过 SSH 启动 Herdr session `{session}` 失败: {error}"
            ))
        })?;
    if output.status != 0 {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("远端命令退出码 {}", output.status)
        } else {
            stderr
        };
        return Err(RuntimeError::message(format!(
            "通过 SSH 启动 Herdr session `{session}` 失败: {detail}"
        )));
    }
    Ok(())
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
            RuntimeCapability::ServerScroll,
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

    fn discover_scoped(
        &self,
        connect: &dyn TargetConnection,
        spec: &RuntimeSpec,
    ) -> RuntimeResult<Vec<ExistingCandidate>> {
        let Some(socket) = spec.socket.as_deref().filter(|_| !spec.session.is_empty()) else {
            return self.discover(
                connect,
                (!spec.session.is_empty()).then_some(spec.session.as_str()),
            );
        };
        // 显式身份必须查询它自己的服务，不能扫描默认目录后误判为缺失。
        let session = HerdrSession::with_connection(
            Connect::new(connect.transport_id(), connect.target()),
            &spec.session,
            socket,
        );
        Ok(session
            .workspace_list()
            .map_err(RuntimeError::message)?
            .into_iter()
            .map(|workspace| ExistingCandidate {
                runtime_id: "herdr".into(),
                transport_id: connect.transport_id().into(),
                target: connect.target().into(),
                namespace: Some(spec.session.clone()),
                name: workspace.label,
                extra: workspace.workspace_id.clone(),
                session: Some(spec.session.clone()),
                socket: Some(socket.into()),
                workspace_id: Some(workspace.workspace_id),
            })
            .collect())
    }

    fn prepare_create(
        &self,
        connect: &dyn TargetConnection,
        spec: &RuntimeSpec,
    ) -> RuntimeResult<Option<String>> {
        if connect.transport_id() != "ssh" {
            return Ok(None);
        }

        let ssh_config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
        let sessions = crate::discovery::existing::ssh_herdr_sessions(
            connect.target(),
            ssh_config.as_deref(),
            HERDR_DISCOVERY_TIMEOUT,
        );
        if spec.session.is_empty()
            && sessions
                .as_ref()
                .is_some_and(|sessions| !sessions.is_empty())
        {
            // A target with existing named sessions needs the normal namespace
            // choice flow; do not create a default server beside them.
            return Ok(None);
        }

        let session = if spec.session.is_empty() {
            "default"
        } else {
            spec.session.as_str()
        };
        let socket = ensure_ssh_herdr_session(connect, session, spec.socket.as_deref())?;
        Ok(Some(socket))
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
            None if connect.transport_id() == "local" => local_herdr_socket_for(session_name),
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
        let session_name = spec.session.as_str();
        let socket = if is_ssh {
            let socket =
                ensure_ssh_herdr_session(connection, session_name, spec.socket.as_deref())?;
            spec.socket = Some(socket.clone());
            socket
        } else {
            spec.socket
                .clone()
                .unwrap_or_else(|| local_herdr_socket_for(session_name))
        };
        let connection = Connect::new(connection.transport_id(), connection.target());
        let herdr = HerdrSession::with_connection(connection, session_name, &socket);
        if herdr.ping().is_err() {
            return Err(RuntimeError::message(format!(
                "Herdr session `{session_name}` 无法通过 socket `{socket}` 响应；请检查远端 Herdr 启动状态"
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
