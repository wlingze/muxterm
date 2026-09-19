//! GTK 主窗口的连接、打开请求和 target 对话框编排。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::Window;

use crate::protocol::WorkspaceId;

use crate::frontend::linux::quickconnect::existing::{ExistingEntry, ExistingTransport};
use crate::frontend::linux::quickconnect::model::{
    QuickConnect, RecentWorkspaceDescriptor, TargetConfigDraft, TargetRuntime, TargetTransport,
};
use crate::frontend::linux::quickconnect::project_flow::ProjectConnectIntent;
use crate::frontend::linux::tmux_dialog::{self, TmuxAction};
use crate::frontend::linux::view_store::ViewStore;
use crate::frontend::utils::corebridge::{
    ClientCandidateRef, ClientOpenIntent, ClientOpenRequest, ClientOpenedWorkspace, ClientTarget,
    FfiClient,
};
use crate::frontend::utils::i18n::{self, Key};

use super::window_event_pump::sync_view_store;
use super::window_scene::after_activate;
use super::{parse_workspace_id, UiState};

/// TargetConfigDraft + session → 稳定 WorkspaceId。
pub(super) fn workspace_id_for_config(config: &TargetConfigDraft, session: &str) -> WorkspaceId {
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

pub(super) fn connect_target(state: &Rc<RefCell<UiState>>, config: TargetConfigDraft) {
    connect_target_with_intent(state, config, ProjectConnectIntent::CreateIfMissing);
}

pub(super) fn connect_target_with_intent(
    state: &Rc<RefCell<UiState>>,
    config: TargetConfigDraft,
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
    if state.borrow().pending_open.is_some() {
        // 一个待打开页面对应一个 Core 操作，重复点击不产生并发连接。
        state
            .borrow()
            .scenes
            .widget()
            .set_visible_child_name("workspace-loading");
        return;
    }
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
        s.event_pump.client().start_open(&request)
    };
    match result {
        Ok(()) => {
            let mut s = state.borrow_mut();
            let stack = s.scenes.widget();
            if let Some(old) = stack.child_by_name("workspace-loading") {
                stack.remove(&old);
            }
            let page = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
            page.set_halign(gtk4::Align::Center);
            page.set_valign(gtk4::Align::Center);
            let spinner = gtk4::Spinner::new();
            spinner.set_size_request(32, 32);
            spinner.start();
            page.append(&spinner);
            let heading = gtk4::Label::new(Some(&i18n::tr(Key::StatusConnecting)));
            heading.add_css_class("title-2");
            page.append(&heading);
            let detail = gtk4::Label::new(Some(&label));
            detail.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            detail.set_max_width_chars(50);
            page.append(&detail);
            let back = gtk4::Button::with_label(&i18n::tr(Key::ExistingBack));
            let weak = Rc::downgrade(state);
            back.connect_clicked(move |_| {
                if let Some(state) = weak.upgrade() {
                    let mut s = state.borrow_mut();
                    let id = s.active_ws_id();
                    super::window_scene::activate_existing(&mut s, id);
                }
            });
            page.append(&back);
            stack.add_named(&page, Some("workspace-loading"));
            s.aggregate.kind = None;
            stack.set_visible_child_name("workspace-loading");
            s.pending_open = Some(PendingWorkspaceOpen {
                label,
                socket: request_socket,
                activate: request.activate,
                shells: false,
            });
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

pub(super) struct PendingWorkspaceOpen {
    label: String,
    socket: Option<String>,
    activate: bool,
    shells: bool,
}

/// 空 Shells 入口与新建 Tab 共用异步打开路径；不在 GTK 线程连接。
pub(super) fn open_local_shell(s: &mut UiState) {
    if s.pending_open.is_some() {
        return;
    }
    use crate::frontend::linux::quickconnect::model::{
        TargetConfigDraft, TargetRuntime, TargetTransport,
    };
    let draft = TargetConfigDraft::new("local", TargetRuntime::Shell, TargetTransport::Local, "~");
    match s.event_pump.client().start_open_target(
        &client_target_from_config(&draft),
        ClientOpenIntent::CreateIfMissing,
    ) {
        Ok(()) => {
            let stack = s.scenes.widget();
            if let Some(old) = stack.child_by_name("workspace-loading") {
                stack.remove(&old);
            }
            let label = gtk4::Label::new(Some(&i18n::tr(Key::StatusConnecting)));
            stack.add_named(&label, Some("workspace-loading"));
            stack.set_visible_child_name("workspace-loading");
            s.pending_open = Some(PendingWorkspaceOpen {
                label: "local shell".into(),
                socket: None,
                activate: true,
                shells: true,
            });
        }
        Err(error) => s.notification_log.push(format!("local shell: {error}")),
    }
}

pub(super) fn poll_pending_open(s: &mut UiState) {
    let Some(pending) = &s.pending_open else {
        return;
    };
    let stack = s.scenes.widget();
    let showing_loading = stack.visible_child_name().as_deref() == Some("workspace-loading");
    let activate = showing_loading && pending.activate;
    let result = s.event_pump.client().poll_open(activate);
    if matches!(result, Ok(None)) {
        return;
    }
    let pending = s.pending_open.take().expect("pending open");
    match result {
        Ok(Some(opened)) => {
            if let Some(id) = parse_workspace_id(&opened.id) {
                s.workspace_sockets.insert(
                    id,
                    pending.socket.or_else(|| opened_workspace_socket(&opened)),
                );
            }
            if let Err(error) = sync_view_store(s) {
                s.notification_log
                    .push(format!("{}: {error}", pending.label));
            } else if activate {
                after_activate(s);
                if pending.shells {
                    super::window_aggregate::show(
                        s,
                        crate::frontend::linux::chrome::aggregate::AggregateKind::Shells,
                    );
                }
            } else if let Some(id) = parse_workspace_id(&opened.id) {
                super::window_scene::ensure_background_scene(s, &id);
                super::window_layout::refresh_workspace_layout(s, &id, true);
            }
        }
        Err(error) => {
            s.notification_log
                .push(format!("{}: {error}", pending.label));
            tracing::warn!(target = "muxterm::linux", %error, "asynchronous workspace open failed");
            if showing_loading {
                if let Some(page) = stack
                    .child_by_name("workspace-loading")
                    .and_then(|page| page.downcast::<gtk4::Box>().ok())
                {
                    let back = page.last_child();
                    if let Some(back) = &back {
                        page.remove(back);
                    }
                    while let Some(child) = page.first_child() {
                        page.remove(&child);
                    }
                    let message = gtk4::Label::new(Some(&format!("{}\n{error}", pending.label)));
                    message.set_wrap(true);
                    message.set_max_width_chars(60);
                    message.set_selectable(true);
                    page.append(&message);
                    if let Some(back) = back {
                        page.append(&back);
                    }
                }
                return;
            }
        }
        Ok(None) => unreachable!(),
    }
    if showing_loading && !activate {
        let id = s.active_ws_id();
        let _ = s.scenes.show(&id);
    }
    if let Some(page) = stack.child_by_name("workspace-loading") {
        stack.remove(&page);
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

fn client_target_from_config(config: &TargetConfigDraft) -> ClientTarget {
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
pub(super) fn recent_workspaces(
    view_store: &ViewStore,
    workspace_sockets: &HashMap<WorkspaceId, Option<String>>,
    limit: usize,
) -> Vec<RecentWorkspaceDescriptor> {
    let mut workspaces: Vec<&crate::frontend::utils::corebridge::ClientWorkspace> = view_store
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
            RecentWorkspaceDescriptor::from_draft(&workspace_to_draft_config(workspace, socket))
        })
        .collect()
}

/// Owned workspace DTO → QuickConnect 目标（Recents 列表 / 面板高亮）。
///
/// 读 `resolved_target().canonical`（Catalog 打开时保存）；无 descriptor 时
/// 从 WorkspaceId 推导（测试 mock/CLI 直开路径）。
pub(super) fn workspace_to_draft_config(
    workspace: &crate::frontend::utils::corebridge::ClientWorkspace,
    tmux_socket: Option<&str>,
) -> TargetConfigDraft {
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
        let mut config = TargetConfigDraft::new(
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
        return TargetConfigDraft::new(
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
    let mut config = TargetConfigDraft::new(name, runtime, transport, id.path.clone());
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
