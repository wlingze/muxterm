//! Non-blocking, create-only application of a workspace template.
//!
//! A template is deliberately applied through the normal Workspace Task path.
//! Structural tasks may be accepted by an asynchronous Runtime, so this module
//! never assumes that dispatch means that the tab or pane already exists.

use std::collections::{HashSet, VecDeque};

use anyhow::Result;

use crate::executable::parse_command_argv;
use crate::protocol::state::{MutationKind, MutationResult, State, StateChange};
use crate::protocol::task::{Task, TaskOutcome};
use crate::protocol::terminal::input::KeyEvent;
use crate::protocol::{PaneId, TabId};
use crate::runtime::{ControlEvent, RuntimeBatch, RuntimeCapability};
use crate::workspace::template::{PaneTemplate, TemplateLayout, TemplateName, WorkspaceTemplate};

/// Why a part of a valid template was not materialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSkip {
    /// The Runtime does not expose the SplitPane capability. The first branch
    /// remains usable and the second branch is intentionally not fabricated.
    SplitPaneUnsupported { tab_index: usize },
}

/// Terminal result of a create-time template application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateApplyReport {
    pub template: TemplateName,
    pub completed: bool,
    pub applied_tabs: usize,
    pub applied_panes: usize,
    pub skipped: Vec<TemplateSkip>,
    pub failed: Option<String>,
}

struct TabState {
    id: Option<TabId>,
    root: Option<PaneId>,
    root_initialized: bool,
    prepared: bool,
}

enum LayoutWork {
    Visit {
        tab_index: usize,
        tab: TabId,
        pane: PaneId,
        layout: TemplateLayout,
        initialized: bool,
    },
}

enum PendingMutation {
    NewTab {
        tab_index: usize,
        operation_id: Option<u64>,
        tab: Option<TabId>,
        pane: Option<PaneId>,
        settled: bool,
        root_initialized: bool,
    },
    SplitPane {
        tab_index: usize,
        parent: PaneId,
        first: Box<TemplateLayout>,
        second: Box<TemplateLayout>,
        operation_id: Option<u64>,
        pane: Option<PaneId>,
        settled: bool,
        second_initialized: bool,
    },
}

/// Poll-driven template state machine owned by one Workspace.
pub struct TemplateApplication {
    template: WorkspaceTemplate,
    report: TemplateApplyReport,
    tabs: Vec<TabState>,
    current_tab: usize,
    work: Vec<LayoutWork>,
    simple_tasks: VecDeque<Task>,
    pending: Option<PendingMutation>,
    known_tabs: HashSet<TabId>,
    known_panes: HashSet<PaneId>,
    focus_target: Option<(usize, PaneId)>,
    finalization_started: bool,
}

impl TemplateApplication {
    pub fn new(template: WorkspaceTemplate) -> Result<Self> {
        template.validate()?;
        let report = TemplateApplyReport {
            template: template.name.clone(),
            completed: false,
            applied_tabs: 0,
            applied_panes: 0,
            skipped: Vec::new(),
            failed: None,
        };
        let tabs = (0..template.tabs.len())
            .map(|_| TabState {
                id: None,
                root: None,
                root_initialized: false,
                prepared: false,
            })
            .collect();
        Ok(Self {
            template,
            report,
            tabs,
            current_tab: 0,
            work: Vec::new(),
            simple_tasks: VecDeque::new(),
            pending: None,
            known_tabs: HashSet::new(),
            known_panes: HashSet::new(),
            focus_target: None,
            finalization_started: false,
        })
    }

    pub fn report(&self) -> &TemplateApplyReport {
        &self.report
    }

    pub fn is_pending(&self) -> bool {
        !self.report.completed
    }

    /// Seed the identity set before issuing a mutation. This prevents the
    /// initial create bootstrap events from being mistaken for a later tab.
    pub(crate) fn bootstrap(&mut self, state: &dyn State) {
        self.remember_state(state);
        self.adopt_initial_topology(state);
    }

    /// Consume the control lane without waiting for I/O.
    pub(crate) fn observe_batch(&mut self, state: &dyn State, batch: &RuntimeBatch) {
        if self.report.completed {
            return;
        }
        for event in &batch.control {
            self.observe_event(event);
        }
        self.adopt_initial_topology(state);
        self.finish_pending_if_ready();
    }

    /// Compatibility adapter for tests and callers that still hold the mixed
    /// state-change view.
    pub(crate) fn observe(&mut self, state: &dyn State, events: &[StateChange]) {
        let batch = RuntimeBatch::from_state_changes(events.iter().cloned());
        self.observe_batch(state, &batch);
    }

    /// Return at most one Runtime Task. A structural task installs a pending
    /// barrier before it is returned, so callers can safely execute it and
    /// then report the resulting TaskOutcome.
    pub(crate) fn next_task(
        &mut self,
        state: &dyn State,
        capabilities: &[RuntimeCapability],
    ) -> Option<Task> {
        if self.report.completed || self.pending.is_some() {
            return None;
        }
        self.adopt_initial_topology(state);

        loop {
            if let Some(task) = self.simple_tasks.pop_front() {
                return Some(task);
            }

            if let Some(LayoutWork::Visit {
                tab_index,
                tab,
                pane,
                layout,
                initialized,
            }) = self.work.pop()
            {
                match layout {
                    TemplateLayout::Pane(template_pane) => {
                        self.report.applied_panes += 1;
                        if template_pane.focus {
                            self.focus_target = Some((tab_index, pane));
                        }
                        if !initialized {
                            if let Some(task) = pane_input_task(pane, &template_pane) {
                                self.simple_tasks.push_back(task);
                            }
                        }
                        continue;
                    }
                    TemplateLayout::Split { dir, first, second } => {
                        if !capabilities.contains(&RuntimeCapability::SplitPane) {
                            self.report
                                .skipped
                                .push(TemplateSkip::SplitPaneUnsupported { tab_index });
                            self.work.push(LayoutWork::Visit {
                                tab_index,
                                tab,
                                pane,
                                layout: *first,
                                initialized: false,
                            });
                            continue;
                        }

                        let (command, workdir) = pane_creation_args(&second);
                        let second_initialized = matches!(second.as_ref(), TemplateLayout::Pane(_));
                        self.pending = Some(PendingMutation::SplitPane {
                            tab_index,
                            parent: pane,
                            first,
                            second,
                            operation_id: None,
                            pane: None,
                            settled: false,
                            second_initialized,
                        });
                        return Some(Task::SplitPane {
                            target: Some(pane),
                            dir,
                            command,
                            workdir,
                        });
                    }
                }
            }

            if self.current_tab < self.template.tabs.len() {
                let tab_index = self.current_tab;
                let tab_template = &self.template.tabs[tab_index];
                let tab_state = &mut self.tabs[tab_index];

                if tab_state.id.is_none() {
                    if tab_index == 0 {
                        return None;
                    }
                    let (command, workdir) = pane_creation_args(&tab_template.layout);
                    let root_initialized = matches!(tab_template.layout, TemplateLayout::Pane(_));
                    self.pending = Some(PendingMutation::NewTab {
                        tab_index,
                        operation_id: None,
                        tab: None,
                        pane: None,
                        settled: false,
                        root_initialized,
                    });
                    return Some(Task::NewTab {
                        name: tab_template.name.clone(),
                        command,
                        workdir,
                    });
                }

                tab_state.root?;

                if !tab_state.prepared {
                    tab_state.prepared = true;
                    self.report.applied_tabs += 1;
                    if let Some(name) = tab_template.name.clone() {
                        self.simple_tasks.push_back(Task::RenameTab {
                            target: tab_state.id.expect("prepared tab must have id"),
                            name,
                        });
                    }
                    self.work.push(LayoutWork::Visit {
                        tab_index,
                        tab: tab_state.id.expect("prepared tab must have id"),
                        pane: tab_state.root.expect("prepared tab must have root"),
                        layout: tab_template.layout.clone(),
                        initialized: tab_state.root_initialized,
                    });
                    continue;
                }

                self.current_tab += 1;
                continue;
            }

            if !self.finalization_started {
                self.finalization_started = true;
                self.queue_final_focus(state);
                continue;
            }

            self.report.completed = true;
            return None;
        }
    }

    pub(crate) fn on_task_outcome(&mut self, outcome: TaskOutcome) {
        if self.report.completed {
            return;
        }
        match outcome {
            TaskOutcome::Done => {
                if let Some(pending) = self.pending.as_mut() {
                    set_pending_settled(pending, true);
                }
            }
            TaskOutcome::Accepted { operation_id } => {
                if let Some(pending) = self.pending.as_mut() {
                    set_pending_operation_id(pending, operation_id);
                } else {
                    self.fail("template received Accepted for a non-mutation task");
                }
            }
            TaskOutcome::Rejected { reason } => {
                self.fail(format!("template task rejected: {reason}"));
            }
        }
    }

    pub(crate) fn on_task_error(&mut self, error: anyhow::Error) {
        self.fail(format!("template task failed: {error:#}"));
    }

    fn observe_event(&mut self, event: &ControlEvent) {
        match event {
            ControlEvent::TabAdded { tab } => {
                let is_new = !self.known_tabs.contains(tab);
                if is_new {
                    match self.pending.as_mut() {
                        Some(PendingMutation::NewTab {
                            tab: pending_tab, ..
                        }) if pending_tab.is_none() => {
                            *pending_tab = Some(*tab);
                        }
                        None if self.tabs[0].id.is_none() => {
                            self.tabs[0].id = Some(*tab);
                        }
                        _ => {}
                    }
                }
                self.known_tabs.insert(*tab);
            }
            ControlEvent::PaneAdded { pane, tab } => {
                let is_new = !self.known_panes.contains(pane);
                if is_new {
                    match self.pending.as_mut() {
                        Some(PendingMutation::NewTab {
                            tab: pending_tab,
                            pane: pending_pane,
                            ..
                        }) if pending_pane.is_none()
                            && (pending_tab.is_none() || *pending_tab == Some(*tab)) =>
                        {
                            *pending_tab = Some(*tab);
                            *pending_pane = Some(*pane);
                        }
                        Some(PendingMutation::SplitPane {
                            tab_index,
                            pane: pending_pane,
                            ..
                        }) if pending_pane.is_none() && self.tabs[*tab_index].id == Some(*tab) => {
                            *pending_pane = Some(*pane);
                        }
                        None if self.tabs[0].id == Some(*tab) && self.tabs[0].root.is_none() => {
                            self.tabs[0].root = Some(*pane);
                        }
                        _ => {}
                    }
                }
                self.known_panes.insert(*pane);
            }
            ControlEvent::MutationSettled {
                operation_id,
                kind,
                result,
            } => {
                let Some(pending) = self.pending.as_mut() else {
                    return;
                };
                let matches = pending_operation_id(pending) == Some(*operation_id)
                    && pending_kind(pending) == *kind;
                if !matches {
                    return;
                }
                match result {
                    MutationResult::Completed => set_pending_settled(pending, true),
                    MutationResult::Failed { reason, .. } => {
                        self.fail(format!("template mutation failed: {reason}"));
                    }
                }
            }
            _ => {}
        }
    }

    fn remember_state(&mut self, state: &dyn State) {
        for tab in state.tabs() {
            self.known_tabs.insert(tab.id);
            for pane in state.panes(&tab.id) {
                self.known_panes.insert(pane.id);
            }
        }
    }

    fn adopt_initial_topology(&mut self, state: &dyn State) {
        if self.tabs[0].id.is_none() {
            self.tabs[0].id = state
                .active_tab()
                .map(|tab| tab.id)
                .or_else(|| state.tabs().first().map(|tab| tab.id));
        }
        let Some(tab_id) = self.tabs[0].id else {
            return;
        };
        if self.tabs[0].root.is_none() {
            let panes = state.panes(&tab_id);
            self.tabs[0].root = panes
                .iter()
                .find(|pane| pane.active)
                .or_else(|| panes.first())
                .map(|pane| pane.id);
        }
    }

    fn finish_pending_if_ready(&mut self) {
        let ready = match self.pending.as_ref() {
            Some(PendingMutation::NewTab {
                tab, pane, settled, ..
            }) => tab.is_some() && pane.is_some() && *settled,
            Some(PendingMutation::SplitPane { pane, settled, .. }) => pane.is_some() && *settled,
            None => false,
        };
        if !ready {
            return;
        }

        let pending = self.pending.take().expect("ready pending mutation exists");
        match pending {
            PendingMutation::NewTab {
                tab_index,
                tab,
                pane,
                root_initialized,
                ..
            } => {
                let tab_state = &mut self.tabs[tab_index];
                tab_state.id = tab;
                tab_state.root = pane;
                tab_state.root_initialized = root_initialized;
            }
            PendingMutation::SplitPane {
                tab_index,
                parent,
                first,
                second,
                pane,
                second_initialized,
                ..
            } => {
                let new_pane = pane.expect("ready split must have new pane");
                let tab = self.tabs[tab_index].id.expect("split tab must have an id");
                self.work.push(LayoutWork::Visit {
                    tab_index,
                    tab,
                    pane: new_pane,
                    layout: *second,
                    initialized: second_initialized,
                });
                self.work.push(LayoutWork::Visit {
                    tab_index,
                    tab,
                    pane: parent,
                    layout: *first,
                    initialized: false,
                });
            }
        }
    }

    fn queue_final_focus(&mut self, state: &dyn State) {
        let desired_index = self
            .template
            .tabs
            .iter()
            .position(|tab| tab.active)
            .unwrap_or(0);
        let Some(desired_tab) = self.tabs.get(desired_index).and_then(|tab| tab.id) else {
            return;
        };
        if state.active_tab().map(|tab| tab.id) != Some(desired_tab) {
            self.simple_tasks.push_back(Task::SwitchTab {
                target: desired_tab,
            });
        }
        if self
            .focus_target
            .is_some_and(|(tab_index, _)| tab_index == desired_index)
        {
            if let Some((_, pane)) = self.focus_target {
                self.simple_tasks
                    .push_back(Task::SwitchPane { target: pane });
            }
        }
    }

    fn fail(&mut self, reason: impl Into<String>) {
        self.pending = None;
        self.work.clear();
        self.simple_tasks.clear();
        self.report.failed = Some(reason.into());
        self.report.completed = true;
    }
}

fn pending_operation_id(pending: &PendingMutation) -> Option<u64> {
    match pending {
        PendingMutation::NewTab { operation_id, .. }
        | PendingMutation::SplitPane { operation_id, .. } => *operation_id,
    }
}

fn pending_kind(pending: &PendingMutation) -> MutationKind {
    match pending {
        PendingMutation::NewTab { .. } => MutationKind::NewTab,
        PendingMutation::SplitPane { .. } => MutationKind::SplitPane,
    }
}

fn set_pending_operation_id(pending: &mut PendingMutation, operation_id: u64) {
    match pending {
        PendingMutation::NewTab {
            operation_id: current,
            ..
        }
        | PendingMutation::SplitPane {
            operation_id: current,
            ..
        } => *current = Some(operation_id),
    }
}

fn set_pending_settled(pending: &mut PendingMutation, settled: bool) {
    match pending {
        PendingMutation::NewTab {
            settled: current, ..
        }
        | PendingMutation::SplitPane {
            settled: current, ..
        } => *current = settled,
    }
}

/// Native pane start for a newly created tab/split: argv + cwd go on the Task.
/// Env is prefixed with `env KEY=VAL` so the runtime spawn path can consume it.
fn pane_creation_args(layout: &TemplateLayout) -> (Option<Vec<String>>, Option<String>) {
    let TemplateLayout::Pane(pane) = layout else {
        return (None, None);
    };
    let mut command = Vec::new();
    if !pane.env.is_empty() {
        command.push("env".to_string());
        command.extend(pane.env.iter().map(|(key, value)| format!("{key}={value}")));
    }
    if let Some(value) = &pane.command {
        command.extend(parse_command_argv(value));
    }
    ((!command.is_empty()).then_some(command), pane.cwd.clone())
}

/// SendKeys fallback for a pane that already exists (create-time root pane).
/// Newly created tabs/splits must use [`pane_creation_args`] on NewTab/SplitPane.
fn pane_input_task(pane: PaneId, template: &PaneTemplate) -> Option<Task> {
    let mut command = String::new();
    if let Some(cwd) = &template.cwd {
        command.push_str("cd ");
        command.push_str(&shell_quote(cwd));
        command.push_str(" && ");
    }
    for (key, value) in &template.env {
        command.push_str("export ");
        command.push_str(key);
        command.push('=');
        command.push_str(&shell_quote(value));
        command.push_str(" && ");
    }
    if let Some(value) = &template.command {
        command.push_str(value);
    }
    if command.is_empty() {
        return None;
    }
    let mut keys = command.chars().map(KeyEvent::Char).collect::<Vec<_>>();
    keys.push(KeyEvent::Enter);
    Some(Task::SendKeys { target: pane, keys })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::layout::LayoutNode;
    use crate::protocol::layout::SplitDir;
    use crate::protocol::TabId;
    use crate::runtime::MockRuntime;

    fn split_template() -> WorkspaceTemplate {
        WorkspaceTemplate {
            name: TemplateName::try_from("dogfood").unwrap(),
            tabs: vec![crate::workspace::template::TabTemplate {
                name: None,
                active: true,
                layout: TemplateLayout::Split {
                    dir: SplitDir::Horizontal,
                    first: Box::new(TemplateLayout::Pane(PaneTemplate {
                        command: None,
                        cwd: None,
                        env: Default::default(),
                        focus: false,
                    })),
                    second: Box::new(TemplateLayout::Pane(PaneTemplate {
                        command: None,
                        cwd: None,
                        env: Default::default(),
                        focus: true,
                    })),
                },
            }],
        }
    }

    #[test]
    fn accepted_split_waits_for_both_topology_and_settlement() {
        let runtime = MockRuntime::with_single_pane();
        let mut app = TemplateApplication::new(split_template()).unwrap();
        app.bootstrap(&runtime);

        let task = app
            .next_task(&runtime, &[RuntimeCapability::SplitPane])
            .expect("template should issue a split");
        assert!(matches!(task, Task::SplitPane { .. }));
        app.on_task_outcome(TaskOutcome::Accepted { operation_id: 7 });

        assert!(
            app.next_task(&runtime, &[RuntimeCapability::SplitPane])
                .is_none(),
            "Accepted must remain pending before events"
        );

        app.observe(
            &runtime,
            &[
                StateChange::PaneAdded {
                    pane: PaneId(2),
                    tab: TabId(1),
                },
                StateChange::MutationSettled {
                    operation_id: 7,
                    kind: MutationKind::SplitPane,
                    result: MutationResult::Completed,
                },
            ],
        );
        let next = app.next_task(&runtime, &[RuntimeCapability::SplitPane]);
        assert!(
            !matches!(next, Some(Task::SplitPane { .. })),
            "settled split must not be re-issued"
        );
        assert!(!app.report().completed || app.report().applied_panes >= 2);
    }

    #[test]
    fn accepted_new_tab_waits_for_tab_pane_and_settlement() {
        let runtime = MockRuntime::with_single_pane();
        let empty_leaf = || {
            TemplateLayout::Pane(PaneTemplate {
                command: None,
                cwd: None,
                env: Default::default(),
                focus: false,
            })
        };
        let command_leaf = || {
            TemplateLayout::Pane(PaneTemplate {
                command: Some("htop".into()),
                cwd: Some("/tmp".into()),
                env: Default::default(),
                focus: false,
            })
        };
        let template = WorkspaceTemplate {
            name: TemplateName::try_from("tabs").unwrap(),
            tabs: vec![
                crate::workspace::template::TabTemplate {
                    name: None,
                    active: true,
                    layout: empty_leaf(),
                },
                crate::workspace::template::TabTemplate {
                    name: Some("second".into()),
                    active: false,
                    layout: command_leaf(),
                },
            ],
        };
        let mut app = TemplateApplication::new(template).unwrap();
        app.bootstrap(&runtime);

        let task = app
            .next_task(&runtime, &[])
            .expect("template should create tab");
        match task {
            Task::NewTab {
                command, workdir, ..
            } => {
                assert_eq!(command.as_deref(), Some(["htop".to_string()].as_slice()));
                assert_eq!(workdir.as_deref(), Some("/tmp"));
            }
            other => panic!("expected native NewTab, got {other:?}"),
        }
        app.on_task_outcome(TaskOutcome::Accepted { operation_id: 9 });
        assert!(
            app.next_task(&runtime, &[]).is_none(),
            "Accepted NewTab must remain pending"
        );

        app.observe(
            &runtime,
            &[
                StateChange::TabAdded { tab: TabId(2) },
                StateChange::PaneAdded {
                    pane: PaneId(2),
                    tab: TabId(2),
                },
                StateChange::MutationSettled {
                    operation_id: 9,
                    kind: MutationKind::NewTab,
                    result: MutationResult::Completed,
                },
            ],
        );
        let next = app.next_task(&runtime, &[]);
        assert!(
            !matches!(next, Some(Task::NewTab { .. })),
            "settled NewTab must not be re-issued"
        );
    }

    #[test]
    fn unsupported_split_is_reported_and_keeps_first_branch() {
        let runtime = MockRuntime::with_single_pane();
        let mut app = TemplateApplication::new(split_template()).unwrap();
        app.bootstrap(&runtime);
        let _ = app.next_task(&runtime, &[]);
        assert_eq!(app.report().skipped.len(), 1);
        assert_eq!(app.report().applied_panes, 1);
    }

    #[test]
    fn initial_topology_can_be_adopted_without_bootstrap_events() {
        let mut runtime = MockRuntime::with_single_pane();
        runtime.layouts[0].tree = LayoutNode::leaf(PaneId(1));
        let mut app = TemplateApplication::new(WorkspaceTemplate {
            name: TemplateName::try_from("one").unwrap(),
            tabs: vec![crate::workspace::template::TabTemplate {
                name: None,
                active: true,
                layout: TemplateLayout::Pane(PaneTemplate {
                    command: None,
                    cwd: None,
                    env: Default::default(),
                    focus: false,
                }),
            }],
        })
        .unwrap();
        app.bootstrap(&runtime);
        assert!(app.next_task(&runtime, &[]).is_none());
        assert_eq!(app.report().applied_tabs, 1);
        assert_eq!(app.report().applied_panes, 1);
    }
}
