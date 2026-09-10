//! GTK 主窗口的连接、打开请求和 target 对话框编排。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::Window;

use muxterm_protocol::WorkspaceId;

use crate::frontend::ffi_client::{
    ClientCandidateRef, ClientOpenIntent, ClientOpenRequest, ClientOpenedWorkspace, ClientTarget,
    FfiClient,
};
use crate::frontend::i18n::{self, Key};
use crate::frontend::linux::quickconnect::existing::{ExistingEntry, ExistingTransport};
use crate::frontend::linux::quickconnect::model::{
    QuickConnect, TargetConfig, TargetRuntime, TargetTransport,
};
use crate::frontend::linux::quickconnect::project_flow::ProjectConnectIntent;
use crate::frontend::linux::tmux_dialog::{self, TmuxAction};
use crate::frontend::linux::view_store::ViewStore;

use super::window_event_pump::sync_view_store;
use super::{after_activate, parse_workspace_id, UiState};

/// TargetConfig + session → 稳定 WorkspaceId。
pub(super) fn workspace_id_for_config(config: &TargetConfig, session: &str) -> WorkspaceId {
    let alias = match &config.transport {
        TargetTransport::Ssh { name } => Some(name.as_str()),
        TargetTransport::Local => None,
    };
    let transport = if config.transport.is_ssh() {
        "ssh"
    } else {
        "local"
    };
    WorkspaceId::new(
        transport,
        alias,
        session,
        config.runtime.as_str(),
        &config.path,
    )
}

pub(super) fn connect_target(state: &Rc<RefCell<UiState>>, config: TargetConfig) {
    connect_target_with_intent(state, config, ProjectConnectIntent::CreateIfMissing);
}

pub(super) fn connect_target_with_intent(
    state: &Rc<RefCell<UiState>>,
    config: TargetConfig,
    intent: ProjectConnectIntent,
) {
    let target = client_target_from_config(&config);
    let open_intent = match intent {
        ProjectConnectIntent::AttachOnly => ClientOpenIntent::AttachOnly,
        ProjectConnectIntent::CreateIfMissing => ClientOpenIntent::CreateIfMissing,
    };
    let result = {
        let s = state.borrow();
        s.event_pump.client().open_target(&target, open_intent)
    };
    match result {
        Ok(opened) => {
            let mut s = state.borrow_mut();
            if let Some(id) = parse_workspace_id(&opened.id) {
                s.workspace_sockets.insert(id, config.socket.clone());
            }
            if let Err(error) = sync_view_store(&mut s) {
                tracing::warn!(target = "muxterm::linux", %error, "workspace snapshot refresh failed after open");
                return;
            }
            after_activate(&mut s);
        }
        Err(error) => {
            let detail = error.to_string();
            tracing::error!(target = "muxterm::linux", "connect failed: {detail}");
            state
                .borrow_mut()
                .notification_log
                .push(format!("{}: connect failed: {detail}", config.name));
        }
    }
}

pub(super) fn connect_open_request(state: &Rc<RefCell<UiState>>, request: ClientOpenRequest) {
    let request_socket = match &request.candidate {
        ClientCandidateRef::Existing { identity } => identity.socket.clone(),
        _ => None,
    };
    let label = match &request.candidate {
        ClientCandidateRef::Project { project_id } => format!("project {project_id}"),
        ClientCandidateRef::Recent { key } => format!("recent {key}"),
        ClientCandidateRef::Existing { identity } => {
            format!("{} @ {}", identity.runtime_id, identity.target)
        }
        _ => "existing connection".to_string(),
    };
    let result = {
        let s = state.borrow();
        s.event_pump.client().open(&request)
    };
    match result {
        Ok(opened) => {
            let socket = request_socket.or_else(|| opened_workspace_socket(&opened));
            let mut s = state.borrow_mut();
            if let Some(id) = parse_workspace_id(&opened.id) {
                s.workspace_sockets.insert(id, socket);
            }
            if let Err(error) = sync_view_store(&mut s) {
                tracing::warn!(
                    target = "muxterm::linux",
                    %error,
                    "workspace snapshot refresh failed after existing attach"
                );
                return;
            }
            after_activate(&mut s);
        }
        Err(error) => {
            let detail = error.to_string();
            tracing::error!(
                target = "muxterm::linux",
                "existing attach failed: {detail}"
            );
            state
                .borrow_mut()
                .notification_log
                .push(format!("{label}: connect failed: {detail}"));
        }
    }
}

fn opened_workspace_socket(opened: &ClientOpenedWorkspace) -> Option<String> {
    opened
        .resolved_target
        .as_ref()
        .and_then(|target| target.get("canonical"))
        .and_then(|canonical| canonical.get("socket"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn client_target_from_config(config: &TargetConfig) -> ClientTarget {
    let (transport, target) = match &config.transport {
        TargetTransport::Local => ("local".to_string(), None),
        TargetTransport::Ssh { name } => ("ssh".to_string(), Some(name.clone())),
    };
    ClientTarget {
        name: config.name.clone(),
        runtime: config.runtime.as_str().to_string(),
        transport,
        target,
        path: config.path.clone(),
        session: config.session.clone(),
        socket: config.socket.clone(),
    }
}

/// 最近打开的工作区 → QuickConnect 目标。
///
/// W6 §11.2：优先读 Core 保存的 `ResolvedTarget.canonical`（含 session /
/// socket / workspace_id）；没有 descriptor 时回退旧五段推导（测试/直开）。
pub(super) fn recent_target_configs(
    view_store: &ViewStore,
    workspace_sockets: &HashMap<WorkspaceId, Option<String>>,
    limit: usize,
) -> Vec<TargetConfig> {
    let mut workspaces: Vec<&crate::frontend::ffi_client::ClientWorkspace> = view_store
        .workspaces()
        .filter_map(|(_, view)| view.workspace.as_ref())
        .collect();
    workspaces.sort_by(|left, right| left.id.cmp(&right.id));
    workspaces
        .into_iter()
        .take(limit)
        .map(|workspace| {
            let id = parse_workspace_id(&workspace.id);
            let socket = id
                .as_ref()
                .and_then(|id| workspace_sockets.get(id))
                .and_then(|value| value.as_deref());
            workspace_to_target_config(workspace, socket)
        })
        .collect()
}

/// Owned workspace DTO → QuickConnect 目标（Recents 列表 / 面板高亮）。
///
/// 读 `resolved_target().canonical`（Catalog 打开时保存）；无 descriptor 时
/// 从 WorkspaceId 推导（测试 mock/CLI 直开路径）。
pub(super) fn workspace_to_target_config(
    workspace: &crate::frontend::ffi_client::ClientWorkspace,
    tmux_socket: Option<&str>,
) -> TargetConfig {
    if let Some(canonical) = workspace
        .resolved_target
        .as_ref()
        .and_then(|resolved| resolved.get("canonical"))
    {
        let name = canonical
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&workspace.name);
        let runtime = canonical
            .get("runtime")
            .and_then(serde_json::Value::as_str)
            .and_then(TargetRuntime::from_str)
            .unwrap_or_else(|| {
                TargetRuntime::from_str(&workspace.runtime).unwrap_or(TargetRuntime::Tmux)
            });
        let transport = match canonical
            .get("transport")
            .and_then(serde_json::Value::as_str)
        {
            Some("ssh") => TargetTransport::Ssh {
                name: canonical
                    .get("target")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            _ => TargetTransport::Local,
        };
        let mut config = TargetConfig::new(
            name,
            runtime,
            transport,
            canonical
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        );
        config.session = canonical
            .get("session")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        config.socket = canonical
            .get("socket")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| tmux_socket.map(str::to_string));
        config.workspace_id = canonical
            .get("workspace_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        return config;
    }
    let Some(id) = parse_workspace_id(&workspace.id) else {
        return TargetConfig::new(
            workspace.name.clone(),
            TargetRuntime::from_str(&workspace.runtime).unwrap_or(TargetRuntime::Tmux),
            TargetTransport::Local,
            "",
        );
    };
    let name = if id.session.is_empty() {
        QuickConnect::default_name(&id.path)
    } else {
        id.session.clone()
    };
    let runtime = TargetRuntime::from_str(&id.runtime).unwrap_or(TargetRuntime::Tmux);
    let transport = if id.transport == "ssh" {
        if let Some(alias) = &id.alias {
            TargetTransport::Ssh {
                name: alias.clone(),
            }
        } else {
            TargetTransport::Local
        }
    } else {
        TargetTransport::Local
    };
    let mut config = TargetConfig::new(name, runtime, transport, id.path.clone());
    if runtime == TargetRuntime::Tmux {
        config.session = (!id.session.is_empty()).then(|| id.session.clone());
        config.socket = tmux_socket.map(str::to_owned);
    }
    config
}

pub(super) fn open_tmux_attach(state: &Rc<RefCell<UiState>>, parent: &Window, _create_only: bool) {
    let active_id = state.borrow().active_ws_id();
    let socket = state
        .borrow()
        .workspace_sockets
        .get(&active_id)
        .cloned()
        .flatten();
    let socket_opt = socket.clone();
    let st = state.clone();
    tmux_dialog::show(parent, socket_opt.as_deref(), move |action| match action {
        TmuxAction::Attach { session } => {
            connect_open_request(
                &st,
                ExistingEntry::tmux(session, ExistingTransport::Local, socket.clone())
                    .open_request(),
            );
        }
        TmuxAction::NewWorkspace { name } => {
            let session = name.unwrap_or_else(|| "muxterm".into());
            let dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            match FfiClient::create_workspace("local", None, socket.as_deref(), &session, &dir) {
                Ok(created) => connect_open_request(
                    &st,
                    ExistingEntry::tmux(created, ExistingTransport::Local, socket.clone())
                        .open_request(),
                ),
                Err(e) => tracing::error!(target = "muxterm::linux", "create tmux session: {e}"),
            }
        }
    });
}

pub(super) fn open_ssh_connect(state: &Rc<RefCell<UiState>>, parent: &Window) {
    let hosts = match FfiClient::discover_ssh_hosts() {
        Ok(h) if !h.is_empty() => h,
        Ok(_) => {
            tracing::error!(
                target = "muxterm::linux",
                "{}",
                i18n::tr(Key::ErrorNoSshHosts)
            );
            return;
        }
        Err(e) => {
            tracing::error!(target = "muxterm::linux", "SSH host discovery failed: {e}");
            return;
        }
    };
    let items = tmux_dialog::connect_pick_items(&hosts);
    let st = state.clone();
    let win = parent.clone();
    crate::frontend::linux::quick_pick::show(
        parent,
        &i18n::tr(Key::ChooseSshHost),
        items,
        move |picked| {
            let Some(item) = picked else {
                return;
            };
            open_connect_sessions(&st, &win, item.id);
        },
    );
}

/// C9：命令面板第二层 = 该 connect 的 runtime list（local 或 SSH alias）。
fn open_connect_sessions(state: &Rc<RefCell<UiState>>, parent: &Window, connect: String) {
    let (transport, target) = if connect == "local" {
        ("local", "")
    } else {
        ("ssh", connect.as_str())
    };
    let sessions = FfiClient::discover_existing(transport, Some(target), None).unwrap_or_default();
    let items = tmux_dialog::connect_session_pick_items(&sessions, &connect);
    let st = state.clone();
    let win = parent.clone();
    let connect_for_attach = connect.clone();
    crate::frontend::linux::quick_pick::show(
        parent,
        &i18n::tr(Key::ChooseWorkspace),
        items,
        move |picked| {
            let Some(item) = picked else {
                return;
            };
            if tmux_dialog::is_create_session_id(&item.id) {
                let connect = connect_for_attach.clone();
                let st = st.clone();
                crate::frontend::linux::pane_switcher::show_rename(&win, "muxterm", move |name| {
                    let dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    let transport = if connect == "local" { "local" } else { "ssh" };
                    let target = if connect == "local" {
                        None
                    } else {
                        Some(connect.as_str())
                    };
                    match FfiClient::create_workspace(transport, target, None, &name, &dir) {
                        Ok(created) => {
                            let transport = if connect == "local" {
                                ExistingTransport::Local
                            } else {
                                ExistingTransport::Ssh {
                                    name: connect.clone(),
                                }
                            };
                            connect_open_request(
                                &st,
                                ExistingEntry::tmux(created, transport, None).open_request(),
                            );
                        }
                        Err(e) => tracing::error!(
                            target = "muxterm::linux",
                            "create remote tmux session: {e}"
                        ),
                    }
                });
            } else {
                let transport = if connect_for_attach == "local" {
                    ExistingTransport::Local
                } else {
                    ExistingTransport::Ssh {
                        name: connect_for_attach.clone(),
                    }
                };
                connect_open_request(
                    &st,
                    ExistingEntry::tmux(item.id, transport, None).open_request(),
                );
            }
        },
    );
}
