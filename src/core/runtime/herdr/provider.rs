//! HerdrDriver：包装 HerdrRuntime + HerdrSession；SSH 走 socket 转发。

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;

use crate::protocol::candidate::ExistingCandidate;
use crate::runtime::herdr::runtime::HerdrRuntime;
use crate::runtime::herdr::session::HerdrSession;
use crate::runtime::RuntimeProvider;
use crate::runtime::{Runtime, RuntimeCapability, RuntimeError, RuntimeResult, RuntimeSpec};
use crate::transport::{ChannelKind, TargetConnection};

/// herdr 插件（local / ssh）。
pub struct HerdrDriver;

fn local_herdr_socket() -> String {
    if let Ok(path) = std::env::var("HERDR_SOCKET_PATH") {
        let path = path.trim();
        if !path.is_empty() {
            return path.to_string();
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.config/herdr/herdr.sock")
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
    ) -> crate::runtime::RuntimeResult<Vec<String>> {
        if connect.transport_id() == "ssh" {
            return Ok(Vec::new());
        }
        // 本地 named sessions 名（不含 default 空串）。
        let mut out = Vec::new();
        let base = std::env::var("HERDR_CONFIG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_default();
                std::path::PathBuf::from(home).join(".config/herdr")
            });
        if let Ok(entries) = std::fs::read_dir(base.join("sessions")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        Ok(out)
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
        if connection.transport_id() == "ssh" {
            return Err(RuntimeError::message(
                "SSH target 不允许启动 workspace.create",
            ));
        }
        let mut spec = spec.clone();
        if spec.session.is_empty() {
            spec.session = "default".into();
        }
        if spec.socket.is_none() {
            spec.socket = Some(local_herdr_socket());
        }
        let socket = spec.socket.as_deref().expect("local herdr socket 已补齐");
        let herdr = HerdrSession::new(&spec.session, socket);
        if herdr.ping().is_err() {
            return Err(RuntimeError::message(format!(
                "Herdr session `{}` 未运行（{}）。先启动 herdr，或从已有连接里选一个 session",
                spec.session, socket
            )));
        }
        let cwd = crate::executable::expand_config_value(&spec.path);
        let created = herdr
            .workspace_create(&cwd, label.unwrap_or(&spec.session))
            .map_err(RuntimeError::message)?;
        spec.path = created.workspace_id;
        Ok(spec)
    }
}
