//! Quick Panel 的 UI→CommandQueue 接线；导航/查询/持久化规则保持可测。
use super::*;
use crate::frontend::gpui::chrome::panel_view::{
    ActionMenu, AggregateKind, PanelRow, PickerAction, RowAction,
};
use crate::frontend::gpui::model::directory::{
    DirectoryListingResponse, DirectorySuggestionController,
};
use crate::frontend::gpui::model::target::{TargetRuntime, TargetTransport};
use crate::frontend::gpui::model::workspace_picker::{
    same_target, CreateDraft, PickerStage, ProjectList,
};
use crate::frontend::utils::corebridge::{
    ClientCandidateRef, ClientExistingCandidateRef, ClientOpenIntent, ClientOpenRequest, CoreBridge,
};

impl GpuiApp {
    pub(super) fn projected_panel_rows(&self) -> Vec<PanelRow> {
        let mut workspaces = self.sidebar_items();
        let mut number = 0;
        for workspace in &mut workspaces {
            if workspace.runtime != "shell" && !workspace.is_aggregate() {
                number += 1;
                workspace.shortcut = (number <= 9).then_some(number);
            }
        }
        let mut rows = self.panel.rows(
            &workspaces,
            self.attention.as_ref(),
            self.search_snapshot.as_ref(),
            &self.existing,
        );
        if self.panel.picker.stage == PickerStage::Root {
            let opened: Vec<_> = self
                .store
                .workspace_ids()
                .filter_map(|id| self.workspace_target(id))
                .collect();
            rows.retain(|row| match row {
                PanelRow::Target { config } | PanelRow::Existing { config, .. } => {
                    !opened.iter().any(|item| same_target(item, config))
                }
                _ => true,
            });
        }
        rows
    }

    pub(super) fn reset_panel_selection(&mut self) {
        let rows = self.panel_rows();
        let query_empty = self.panel.model.query.trim().is_empty();
        self.panel.selected = rows
            .iter()
            .position(|row| match (&self.panel.picker.stage, row) {
                (PickerStage::Root, PanelRow::Workspace { active, .. }) => query_empty && *active,
                (PickerStage::Hosts, PanelRow::Action(PickerAction::Host(_))) => true,
                (PickerStage::Sessions(_), PanelRow::Existing { .. }) => !query_empty,
                (PickerStage::Sessions(_), PanelRow::Action(PickerAction::OpenNewPath)) => {
                    query_empty
                }
                (PickerStage::Path(_), PanelRow::Action(PickerAction::OpenThisDirectory)) => true,
                (PickerStage::Confirm(_), PanelRow::Action(PickerAction::Create)) => true,
                _ => false,
            })
            .unwrap_or(0);
        self.panel_scroll.scroll_to_top_of_item(0);
    }

    pub fn select_panel_row(&mut self, index: usize, cx: &mut Context<Self>) {
        self.panel.selected = index.min(self.panel_rows().len().saturating_sub(1));
        self.activate_panel_row(cx);
    }
    pub fn panel_stage(&self) -> &PickerStage {
        &self.panel.picker.stage
    }
    pub fn panel_scroll_handle(&self) -> &gpui::ScrollHandle {
        &self.panel_scroll
    }
    pub fn panel_placeholder(&self) -> &'static str {
        if self.panel.actions.is_some() {
            return "Type an action or command…";
        }
        match self.panel.picker.stage {
            PickerStage::Root => "Quick Connect",
            PickerStage::Hosts => "Filter machines",
            PickerStage::Sessions(_) => "Filter tmux and herdr",
            PickerStage::Path(_) => "Filter path",
            PickerStage::Confirm(_) => "Workspace name",
        }
    }

    pub(super) fn edit_panel_query(&mut self, text: Option<&str>, cx: &mut Context<Self>) {
        if let Some(menu) = self.panel.actions.as_mut() {
            if let Some(text) = text {
                menu.query.push_str(text);
            } else {
                menu.query.pop();
            }
            menu.selected = 0;
        } else {
            if let Some(text) = text {
                self.panel.model.query.push_str(text);
            } else {
                self.panel.model.query.pop();
            }
            if let PickerStage::Confirm(draft) = &mut self.panel.picker.stage {
                draft.name = self.panel.model.query.clone();
            } else if matches!(self.panel.picker.stage, PickerStage::Path(_)) {
                self.panel.picker.path.update_input(&self.panel.model.query);
                self.refresh_picker_directories(cx);
            }
            self.reset_panel_selection();
        }
        cx.notify();
    }

    pub fn toggle_panel_actions(&mut self, cx: &mut Context<Self>) {
        if self.panel.actions.take().is_none() {
            if let Some(target) = self.panel_rows().get(self.panel.selected).cloned() {
                self.panel.actions = Some(ActionMenu {
                    target,
                    query: String::new(),
                    selected: 0,
                });
            }
        }
        cx.notify();
    }
    pub fn panel_actions(&self) -> Option<&ActionMenu> {
        self.panel.actions.as_ref()
    }
    pub fn select_panel_action(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.panel.actions {
            menu.selected = index;
        }
        self.run_panel_action(cx);
    }
    pub(super) fn run_panel_action(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.panel.actions.as_ref() else {
            return;
        };
        let Some(action) = menu.rows().get(menu.selected).copied() else {
            return;
        };
        let target = menu.target.clone();
        self.panel.actions = None;
        match action {
            RowAction::Open | RowAction::Attach | RowAction::Jump => {
                self.activate_workspace_row(target, cx)
            }
            RowAction::Connect => {
                // Connect 就是"打开这个已存 target"；不新建、不改配置。
                self.activate_workspace_row(target, cx)
            }
            RowAction::Close => {
                if let PanelRow::Workspace { workspace_id, .. } = target {
                    self.close_workspace(workspace_id, cx);
                }
            }
            RowAction::Save => {
                if let PanelRow::Existing { config, .. } = target {
                    self.persist_panel_project(&config, false, cx);
                }
            }
            RowAction::Delete => {
                if let Some(config) = project_config(&target) {
                    self.persist_panel_project(&config, true, cx);
                }
            }
            RowAction::Edit => {
                if let Some(config) = project_config(&target) {
                    self.open_target_editor(Some(&config), cx);
                }
            }
            RowAction::Hide => {
                if let PanelRow::Attention {
                    workspace_id,
                    pane_id,
                    ..
                } = target
                {
                    self.hidden_commands.insert(
                        crate::frontend::gpui::chrome::sidebar_view::CommandKey {
                            workspace_id,
                            pane_id,
                        },
                    );
                }
            }
        }
        cx.notify();
    }
    pub fn save_panel_existing(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(PanelRow::Existing { config, .. }) = self.panel_rows().get(index).cloned() {
            self.persist_panel_project(&config, false, cx);
        }
    }
    pub(super) fn persist_panel_project(
        &mut self,
        config: &TargetConfig,
        remove: bool,
        cx: &mut Context<Self>,
    ) {
        if self.projects_pending {
            return;
        }
        let mut projects = self.panel.picker.projects.clone();
        if remove {
            projects.remove(config);
        } else {
            projects.upsert(config);
        }
        self.commands.push(ClientCommand::ConfigApply {
            path: "projects".into(),
            value: projects.value(),
        });
        // 不乐观显示 Saved：事件泵收到成功提交后再从 Core describe 更新。
        self.projects_pending = true;
        cx.notify();
    }

    pub(super) fn activate_workspace_row(&mut self, row: PanelRow, cx: &mut Context<Self>) {
        if self.open_pending {
            return;
        }
        match row {
            PanelRow::Aggregate(kind) => {
                self.select_aggregate(kind, cx);
                self.close_panel(cx);
            }
            PanelRow::Workspace { workspace_id, .. } => {
                self.select_workspace(workspace_id, cx);
                self.close_panel(cx);
            }
            PanelRow::Attention {
                workspace_id,
                pane_id,
                ..
            } => {
                self.select_workspace(workspace_id, cx);
                self.focus_pane(pane_id, cx);
                self.close_panel(cx);
            }
            PanelRow::Search(hit) => {
                self.select_workspace(hit.workspace_id, cx);
                self.focus_pane(hit.pane_id, cx);
                self.close_panel(cx);
            }
            PanelRow::Target { config } => {
                let Some(project_id) = config.persisted_id else {
                    return;
                };
                self.queue_panel_candidate(
                    ClientCandidateRef::Project { project_id },
                    ClientOpenIntent::CreateIfMissing,
                    cx,
                );
            }
            PanelRow::Existing { config, .. } => {
                let (transport_id, target) = match config.transport {
                    TargetTransport::Local => ("local".into(), String::new()),
                    TargetTransport::Ssh { name } => ("ssh".into(), name),
                };
                let identity = ClientExistingCandidateRef {
                    runtime_id: config.runtime.label().to_lowercase(),
                    transport_id,
                    target,
                    session: config.session,
                    socket: config.socket,
                    workspace_id: config.workspace_id,
                };
                self.queue_panel_candidate(
                    ClientCandidateRef::Existing { identity },
                    ClientOpenIntent::AttachOnly,
                    cx,
                );
            }
            PanelRow::Action(action) => self.navigate_picker(action, cx),
            PanelRow::Status(_) => {}
        }
    }
    fn queue_panel_candidate(
        &mut self,
        candidate: ClientCandidateRef,
        intent: ClientOpenIntent,
        cx: &mut Context<Self>,
    ) {
        self.commands.push(ClientCommand::OpenCandidate {
            request: ClientOpenRequest {
                candidate,
                intent,
                template: None,
                activate: true,
            },
        });
        self.open_pending = true;
        self.panel.picker.error = None;
        cx.notify();
    }

    pub fn begin_panel_open(&mut self, cx: &mut Context<Self>) {
        self.panel
            .show(crate::frontend::gpui::model::panel::PanelTab::Workspaces);
        self.navigate_picker(PickerAction::Open, cx);
    }
    fn navigate_picker(&mut self, action: PickerAction, cx: &mut Context<Self>) {
        match action {
            PickerAction::Open => {
                self.panel.picker.navigate(PickerStage::Hosts);
                self.panel.model.query.clear();
                self.discover_picker_aliases(cx);
            }
            PickerAction::Back => {
                self.panel.picker.back();
                self.panel.model.query = if matches!(self.panel.picker.stage, PickerStage::Path(_))
                {
                    self.panel.picker.path.text().into()
                } else {
                    String::new()
                };
                if matches!(self.panel.picker.stage, PickerStage::Sessions(_)) {
                    self.discover_picker_sessions(cx);
                }
            }
            PickerAction::Host(transport) => {
                self.panel.picker.enter_sessions(transport);
                self.panel.model.query.clear();
                self.discover_picker_sessions(cx);
            }
            PickerAction::OpenNewPath => {
                if let PickerStage::Sessions(transport) = self.panel.picker.stage.clone() {
                    self.panel
                        .picker
                        .navigate(PickerStage::Path(transport.clone()));
                    self.panel.picker.path = DirectorySuggestionController::new("~");
                    let alias = match &transport {
                        TargetTransport::Local => None,
                        TargetTransport::Ssh { name } => Some(name.as_str()),
                    };
                    self.panel.picker.path.set_transport(alias.is_some(), alias);
                    self.panel.model.query = "~".into();
                    self.refresh_picker_directories(cx);
                }
            }
            PickerAction::Directory(name) => {
                self.panel.picker.path.select(&name);
                self.panel.model.query = self.panel.picker.path.text().into();
                self.refresh_picker_directories(cx);
            }
            PickerAction::OpenThisDirectory => {
                if let PickerStage::Path(transport) = self.panel.picker.stage.clone() {
                    let draft = CreateDraft::new(transport, self.panel.picker.path.text());
                    self.panel.model.query = draft.name.clone();
                    self.panel.picker.navigate(PickerStage::Confirm(draft));
                }
            }
            PickerAction::Create => {
                if let PickerStage::Confirm(draft) = &self.panel.picker.stage {
                    match draft.config() {
                        Ok(config) => {
                            self.commands.push(ClientCommand::OpenTarget {
                                target: super::client_target_for(&config),
                                intent: ClientOpenIntent::CreateIfMissing,
                            });
                            self.open_pending = true;
                            self.panel.picker.error = None;
                        }
                        Err(error) => self.panel.picker.error = Some(error.into()),
                    }
                }
            }
        }
        self.reset_panel_selection();
        cx.notify();
    }
    pub fn choose_panel_runtime(&mut self, runtime: TargetRuntime, cx: &mut Context<Self>) {
        if let PickerStage::Confirm(draft) = &mut self.panel.picker.stage {
            if matches!(runtime, TargetRuntime::Herdr | TargetRuntime::Tmux) {
                draft.runtime = runtime;
            }
        }
        cx.notify();
    }
    pub fn set_panel_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.panel.actions {
            menu.query.clear();
        } else {
            self.panel.model.query.clear();
        }
        self.edit_panel_query(Some(query), cx);
    }
    pub(super) fn route_panel_key(&mut self, key: &gpui::Keystroke, cx: &mut Context<Self>) {
        if key.modifiers.platform && key.key == "k" {
            self.toggle_panel_actions(cx);
            return;
        }
        if key.modifiers.platform {
            if key.key == "a" {
                self.set_panel_query("", cx);
            }
            if key.key == "v" {
                if let Some(item) = cx.read_from_clipboard() {
                    let text = item.entries().iter().find_map(|entry| match entry {
                        gpui::ClipboardEntry::String(text) => Some(text.text().to_string()),
                        _ => None,
                    });
                    if let Some(text) = text {
                        self.panel_push_query(&text, cx);
                    }
                }
            }
            return;
        }
        match key.key.as_str() {
            "escape" => {
                if self.panel.actions.take().is_some() {
                    cx.notify();
                } else if self.panel.picker.stage != PickerStage::Root {
                    self.navigate_picker(PickerAction::Back, cx);
                } else {
                    self.close_panel(cx);
                }
            }
            "enter" => self.activate_panel_row(cx),
            "up" => self.move_panel_selection(-1, cx),
            "down" => self.move_panel_selection(1, cx),
            "p" | "n" if key.modifiers.control => {
                self.move_panel_selection(if key.key == "n" { 1 } else { -1 }, cx)
            }
            "backspace" => self.panel_backspace(cx),
            "tab" => {
                if self.panel.actions.is_some() {
                    return;
                }
                if matches!(
                    self.panel.picker.stage,
                    PickerStage::Hosts | PickerStage::Path(_)
                ) {
                    if let Some(row) = self.panel_rows().get(self.panel.selected).cloned() {
                        match row {
                            PanelRow::Action(PickerAction::Host(host)) => {
                                self.set_panel_query(host.label(), cx)
                            }
                            PanelRow::Action(PickerAction::Directory(name)) => {
                                self.navigate_picker(PickerAction::Directory(name), cx)
                            }
                            _ => {}
                        }
                    }
                } else if let PickerStage::Confirm(draft) = &self.panel.picker.stage {
                    self.choose_panel_runtime(
                        if draft.runtime == TargetRuntime::Herdr {
                            TargetRuntime::Tmux
                        } else {
                            TargetRuntime::Herdr
                        },
                        cx,
                    );
                } else {
                    self.cycle_panel_tab(key.modifiers.shift, cx);
                    self.reset_panel_selection();
                }
            }
            _ if !key.modifiers.control && !key.modifiers.alt => {
                let text = key.key_char.as_deref().or_else(|| {
                    if key.key == "space" {
                        Some(" ")
                    } else if key.key.chars().count() == 1 {
                        Some(key.key.as_str())
                    } else {
                        None
                    }
                });
                if let Some(text) = text.filter(|text| text.chars().all(|ch| !ch.is_control())) {
                    self.panel_push_query(text, cx);
                }
            }
            _ => {}
        }
    }

    pub(super) fn discover_picker_root(&mut self, cx: &mut Context<Self>) {
        if self.existing_loading || self.existing_loaded || self.pump.is_none() {
            return;
        }
        self.existing_loading = true;
        let socket = self.socket.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { discover_rows(None, socket.as_deref()) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.existing_loading = false;
                app.existing_loaded = true;
                match result {
                    Ok(rows) => app.existing = rows,
                    Err(error) => {
                        app.panel.picker.error = Some(format!("Discovery failed: {error}"))
                    }
                }
                app.reset_panel_selection();
                cx.notify();
            });
        })
        .detach();
    }
    fn discover_picker_aliases(&mut self, cx: &mut Context<Self>) {
        if self.pump.is_none() {
            return;
        }
        let request = self.panel.picker.request();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { CoreBridge::discover_ssh_hosts() })
                .await;
            let _ = this.update(cx, |app, cx| {
                if !app.panel.open || !app.panel.picker.accepts(&request) {
                    return;
                }
                match result {
                    Ok(hosts) => {
                        app.panel.picker.aliases =
                            hosts.into_iter().map(|host| host.alias).collect()
                    }
                    Err(error) => app.panel.picker.error = Some(error.to_string()),
                }
                app.reset_panel_selection();
                cx.notify();
            });
        })
        .detach();
    }
    fn discover_picker_sessions(&mut self, cx: &mut Context<Self>) {
        if self.pump.is_none() {
            return;
        }
        let PickerStage::Sessions(transport) = self.panel.picker.stage.clone() else {
            return;
        };
        let request = self.panel.picker.request();
        self.panel.picker.loading = true;
        let socket = matches!(transport, TargetTransport::Local)
            .then(|| self.socket.clone())
            .flatten();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { discover_rows(Some(&transport), socket.as_deref()) })
                .await;
            let _ = this.update(cx, |app, cx| {
                if !app.panel.open || !app.panel.picker.accepts(&request) {
                    return;
                }
                app.panel.picker.loading = false;
                match result {
                    Ok(rows) => app.panel.picker.sessions = rows,
                    Err(error) => {
                        app.panel.picker.error = Some(format!("Discovery failed: {error}"))
                    }
                }
                app.reset_panel_selection();
                cx.notify();
            });
        })
        .detach();
    }
    fn refresh_picker_directories(&mut self, cx: &mut Context<Self>) {
        if self.pump.is_none() {
            return;
        }
        let request = self.panel.picker.path.request();
        let navigation = self.panel.picker.request();
        cx.spawn(async move |this, cx| {
            let query = request.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    CoreBridge::list_dir(
                        if query.is_ssh { "ssh" } else { "local" },
                        query.alias.as_deref(),
                        &query.path,
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if !app.panel.open || !app.panel.picker.accepts(&navigation) {
                    return;
                }
                match result {
                    Ok(entries) => {
                        app.panel.picker.path.apply(&DirectoryListingResponse {
                            request,
                            directories: entries
                                .into_iter()
                                .filter(|entry| entry.is_dir)
                                .map(|entry| entry.name)
                                .collect(),
                        });
                    }
                    Err(error) => {
                        app.panel.picker.error = Some(format!("Cannot list directory: {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn poll_panel_operations(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pump) = self.pump.as_ref() else {
            return false;
        };
        let mut dirty = false;
        if self.projects_pending {
            self.projects_pending = false;
            if let Ok(snapshot) = pump.client().config_describe() {
                self.panel.picker.projects = ProjectList::new(&snapshot.values["projects"]);
                dirty = true;
            }
        }
        if !self.open_pending {
            return dirty;
        }
        match pump.client().poll_open(true) {
            Ok(None) => {}
            Ok(Some(workspace)) => {
                self.open_pending = false;
                if let Err(error) = pump.sync_view_store(&mut self.store) {
                    tracing::warn!(%error, "sync after open");
                }
                self.visible = Some(workspace.id);
                self.aggregate = None;
                self.focus_default_pane();
                self.last_sizes.clear();
                self.last_client_size = None;
                self.close_panel(cx);
                dirty = true;
            }
            Err(error) => {
                self.open_pending = false;
                self.panel.picker.error = Some(format!("Open failed: {error}"));
                dirty = true;
            }
        }
        dirty
    }
    pub fn panel_operation_pending(&self) -> bool {
        self.open_pending || self.projects_pending
    }

    pub(super) fn workspace_target(&self, id: &str) -> Option<TargetConfig> {
        let workspace = self.store.workspace(id)?.workspace.as_ref()?;
        let canonical = &workspace.resolved_target.as_ref()?["canonical"];
        let transport = if canonical["transport"] == "ssh" {
            TargetTransport::Ssh {
                name: canonical["alias"].as_str().unwrap_or_default().into(),
            }
        } else {
            TargetTransport::Local
        };
        Some(TargetConfig {
            name: canonical["name"].as_str().unwrap_or(&workspace.name).into(),
            runtime: TargetRuntime::parse(&workspace.runtime)?,
            transport,
            path: canonical["path"].as_str().unwrap_or_default().into(),
            session: canonical["session"].as_str().map(str::to_string),
            socket: canonical["socket"].as_str().map(str::to_string),
            workspace_id: canonical["workspace_id"].as_str().map(str::to_string),
            persisted_id: None,
        })
    }

    pub fn aggregate_kind(&self) -> Option<AggregateKind> {
        self.aggregate
    }
    pub(super) fn aggregate_tab_items(
        &self,
    ) -> Option<Vec<crate::frontend::gpui::chrome::tab_bar_view::TabBarItem>> {
        self.aggregate?;
        Some(
            self.aggregate_pages()
                .iter()
                .enumerate()
                .map(|(index, (workspace, tab, title))| {
                    crate::frontend::gpui::chrome::tab_bar_view::TabBarItem {
                        tab_id: (index + 1) as u32,
                        index: (index + 1) as u32,
                        title: title.clone(),
                        active: self.visible.as_ref() == Some(workspace)
                            && self
                                .store
                                .workspace(workspace)
                                .and_then(|view| view.active_tab_id())
                                == Some(*tab),
                        pane_count: self
                            .store
                            .workspace(workspace)
                            .map(|view| view.panes_for_tab(*tab).len())
                            .unwrap_or(0),
                        activity: None,
                    }
                })
                .collect(),
        )
    }
    pub(super) fn select_aggregate_tab(&mut self, display_id: u32, cx: &mut Context<Self>) -> bool {
        if self.aggregate.is_none() {
            return false;
        }
        if let Some((workspace, tab, _)) = self
            .aggregate_pages()
            .get(display_id.saturating_sub(1) as usize)
            .cloned()
        {
            self.visible = Some(workspace.clone());
            self.commands
                .push(ClientCommand::ActivateWorkspace(workspace.clone()));
            self.commands.push(ClientCommand::Task {
                workspace_id: Some(workspace),
                task: crate::frontend::utils::corebridge::ClientTask::SwitchTab { tab_id: tab },
            });
            self.focus_default_pane();
            self.last_sizes.clear();
            cx.notify();
        }
        true
    }
    pub fn aggregate_pages(&self) -> Vec<(String, u32, String)> {
        use crate::frontend::gpui::model::{aggregate, sidebar};
        match self.aggregate {
            None => vec![],
            Some(AggregateKind::Shells) => {
                let sources: Vec<_> = self
                    .store
                    .workspace_ids()
                    .enumerate()
                    .filter_map(|(order, id)| {
                        let workspace = self.store.workspace(id)?;
                        if workspace.workspace.as_ref()?.runtime != "shell" {
                            return None;
                        }
                        Some(aggregate::ShellAggregateSource {
                            workspace_id: id.into(),
                            transport: workspace
                                .workspace
                                .as_ref()
                                .map(super::transport_label)
                                .unwrap_or_default(),
                            opened_order: order as u64,
                            tabs: workspace
                                .tabs
                                .iter()
                                .map(|tab| aggregate::AggregateSourceTab {
                                    id: tab.id,
                                    title: tab.name.clone(),
                                })
                                .collect(),
                        })
                    })
                    .collect();
                aggregate::shell_tabs(&sources)
                    .into_iter()
                    .map(|tab| (tab.workspace_id, tab.source_tab_id, tab.title))
                    .collect()
            }
            Some(AggregateKind::Agents) => aggregate::agent_tabs(
                &sidebar::agents(&self.sidebar_items(), self.attention.as_ref()),
                &self.workspace_ids(),
            )
            .into_iter()
            .map(|tab| (tab.workspace_id, tab.source_tab_id, tab.title))
            .collect(),
        }
    }
    pub fn select_aggregate(&mut self, kind: AggregateKind, cx: &mut Context<Self>) {
        self.aggregate = Some(kind);
        if let Some((workspace, tab, _)) = self.aggregate_pages().first().cloned() {
            // 固定聚合 ID 从不进入 Core；仅激活真实源 workspace/tab。
            self.visible = Some(workspace.clone());
            self.commands
                .push(ClientCommand::ActivateWorkspace(workspace.clone()));
            self.commands.push(ClientCommand::Task {
                workspace_id: Some(workspace),
                task: crate::frontend::utils::corebridge::ClientTask::SwitchTab { tab_id: tab },
            });
            self.focus_default_pane();
        }
        cx.notify();
    }

    #[doc(hidden)]
    pub fn set_picker_data_for_tests(&mut self, projects: serde_json::Value, aliases: Vec<String>) {
        self.panel.picker.projects = ProjectList::new(&projects);
        self.panel.picker.aliases = aliases;
    }
    #[doc(hidden)]
    pub fn set_host_sessions_for_tests(
        &mut self,
        sessions: Vec<crate::frontend::gpui::model::existing::ExistingRuntime>,
    ) {
        self.panel.picker.sessions = sessions;
    }
    #[doc(hidden)]
    pub fn apply_picker_directories_for_tests(&mut self, directories: Vec<String>) -> bool {
        self.panel.picker.path.apply(&DirectoryListingResponse {
            request: self.panel.picker.path.request(),
            directories,
        })
    }
}

fn discover_rows(
    transport: Option<&TargetTransport>,
    socket: Option<&str>,
) -> anyhow::Result<Vec<crate::frontend::gpui::model::existing::ExistingRuntime>> {
    use crate::frontend::gpui::model::existing::ExistingRuntime;
    let (kind, alias) = match transport {
        None => ("all", None),
        Some(TargetTransport::Local) => ("local", None),
        Some(TargetTransport::Ssh { name }) => ("ssh", Some(name.as_str())),
    };
    let mut rows = ExistingRuntime::collect(&CoreBridge::discover_existing(kind, alias, None)?);
    if let Some(socket) = socket {
        rows.extend(
            CoreBridge::discover_tmux_sessions("local", None, Some(socket))?
                .iter()
                .map(|entry| ExistingRuntime::from_tmux_session(entry, Some(socket))),
        );
    }
    let mut seen = std::collections::HashSet::new();
    rows.retain(|row| {
        seen.insert((
            row.transport.clone(),
            row.target.clone(),
            row.runtime.clone(),
            row.session.clone(),
            row.socket.clone(),
            row.workspace_id.clone(),
        ))
    });
    Ok(rows)
}

/// 已保存 Project 的配置：列表行与已保存的现有 runtime 共用同一套行内动作。
fn project_config(row: &PanelRow) -> Option<TargetConfig> {
    match row {
        PanelRow::Target { config } => Some(config.clone()),
        PanelRow::Existing {
            config,
            saved: true,
            ..
        } => Some(config.clone()),
        _ => None,
    }
}
