//! Frontend-owned, workspace-keyed view snapshots.
//!
//! `ViewStore` contains only owned DTOs.  Core never receives a reference to
//! this store; the future `EventPump` (and the current compatibility event
//! adapter) are the only writers.  Render mailboxes are bounded so a chatty
//! hidden pane cannot grow frontend memory without limit.

use std::collections::{HashMap, VecDeque};

use crate::ffi_client::{
    ClientEvent, ClientEventKind, ClientLayout, ClientPane, ClientTab, ClientWorkspace,
    ClientWorkspaceEvent,
};

const RENDER_MAILBOX_CAPACITY: usize = 128;
const COALESCED_OUTPUT_MAX_BYTES: usize = 64 * 1024;

/// Delivery policy for a pane's render-data mailbox.
///
/// `Live` preserves every event boundary. `Coalesce` preserves bytes while
/// merging adjacent output events. `Pause` keeps history and the newest full
/// baseline, while dropping incremental output until the pane is resumed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PaneRenderPolicy {
    #[default]
    Live,
    Coalesce,
    Pause,
}

/// One owned workspace view snapshot.
#[derive(Debug, Default)]
pub struct WorkspaceView {
    pub workspace: Option<ClientWorkspace>,
    pub tabs: Vec<ClientTab>,
    pub panes: HashMap<u32, Vec<ClientPane>>,
    /// One owned layout tree per tab. Layouts are part of the control snapshot
    /// so a renderer never has to query Core while switching scenes.
    pub layouts: HashMap<u32, ClientLayout>,
    render_mailboxes: HashMap<u32, VecDeque<ClientEvent>>,
    render_policies: HashMap<u32, PaneRenderPolicy>,
    pending_baselines: std::collections::HashSet<u32>,
}

/// Owned snapshots and render mailboxes keyed by stable workspace identity.
#[derive(Debug, Default)]
pub struct ViewStore {
    workspaces: HashMap<String, WorkspaceView>,
    order: Vec<String>,
}

impl ViewStore {
    pub fn ensure_workspace(&mut self, workspace_id: &str) -> &mut WorkspaceView {
        if !self.workspaces.contains_key(workspace_id) {
            self.order.push(workspace_id.to_string());
        }
        self.workspaces.entry(workspace_id.to_string()).or_default()
    }

    pub fn remove_workspace(&mut self, workspace_id: &str) -> Option<WorkspaceView> {
        let removed = self.workspaces.remove(workspace_id);
        if removed.is_some() {
            self.order.retain(|id| id != workspace_id);
        }
        removed
    }

    pub fn workspace(&self, workspace_id: &str) -> Option<&WorkspaceView> {
        self.workspaces.get(workspace_id)
    }

    pub fn workspace_ids(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(String::as_str)
    }

    /// Return all owned workspace views in their stable store order.
    pub fn workspaces(&self) -> impl Iterator<Item = (&str, &WorkspaceView)> {
        self.order.iter().filter_map(|workspace_id| {
            self.workspaces
                .get(workspace_id)
                .map(|view| (workspace_id.as_str(), view))
        })
    }

    /// Return the workspace marked active by the latest Core topology snapshot.
    pub fn active_workspace_id(&self) -> Option<&str> {
        self.workspaces().find_map(|(workspace_id, view)| {
            view.workspace
                .as_ref()
                .is_some_and(|workspace| workspace.active)
                .then_some(workspace_id)
        })
    }

    pub fn replace_topology(
        &mut self,
        workspace: ClientWorkspace,
        tabs: Vec<ClientTab>,
        panes: Vec<(u32, Vec<ClientPane>)>,
    ) {
        let view = self.ensure_workspace(&workspace.id);
        view.workspace = Some(workspace);
        view.tabs = tabs;
        view.panes = panes.into_iter().collect();
    }

    /// Replace all tab layouts in one owned control snapshot.
    pub fn replace_layouts(&mut self, workspace_id: &str, layouts: Vec<(u32, ClientLayout)>) {
        self.ensure_workspace(workspace_id).layouts = layouts.into_iter().collect();
    }

    /// Set the delivery policy for one pane and return the previous policy.
    pub fn set_pane_render_policy(
        &mut self,
        workspace_id: &str,
        pane_id: u32,
        policy: PaneRenderPolicy,
    ) -> PaneRenderPolicy {
        let view = self.ensure_workspace(workspace_id);
        let previous = view
            .render_policies
            .insert(pane_id, policy)
            .unwrap_or_default();
        if previous == PaneRenderPolicy::Pause && policy != PaneRenderPolicy::Pause {
            view.pending_baselines.insert(pane_id);
        }
        previous
    }

    pub fn pane_render_policy(&self, workspace_id: &str, pane_id: u32) -> PaneRenderPolicy {
        self.workspaces
            .get(workspace_id)
            .and_then(|view| view.render_policies.get(&pane_id).copied())
            .unwrap_or_default()
    }

    /// Retain only render-data events. Control events are represented by the
    /// owned topology snapshot and do not belong in a pane byte mailbox.
    pub fn push_render_event(&mut self, workspace_id: &str, event: ClientEvent) {
        if !matches!(
            event.kind(),
            ClientEventKind::PaneOutput
                | ClientEventKind::PaneFrame
                | ClientEventKind::PaneSnapshot
                | ClientEventKind::PaneHistory
        ) {
            return;
        }
        let view = self.ensure_workspace(workspace_id);
        let policy = view
            .render_policies
            .get(&event.pane_id)
            .copied()
            .unwrap_or_default();
        let pending_baseline = view.pending_baselines.contains(&event.pane_id);
        let mailbox = view.render_mailboxes.entry(event.pane_id).or_default();
        if pending_baseline {
            push_paused(mailbox, event);
        } else {
            match policy {
                PaneRenderPolicy::Live => push_bounded(mailbox, event),
                PaneRenderPolicy::Coalesce => push_coalesced(mailbox, event),
                PaneRenderPolicy::Pause => push_paused(mailbox, event),
            }
        }
    }

    /// Apply one owned FFI event without exposing the C event buffer to the
    /// store. Control/topology events are refreshed by `EventPump`; only
    /// render-data events enter the pane mailbox here.
    pub fn apply_workspace_event(&mut self, event: ClientWorkspaceEvent) {
        let workspace_id = event.workspace_id;
        self.push_render_event(&workspace_id, event.event);
    }

    pub fn take_pane_render_events(
        &mut self,
        workspace_id: &str,
        pane_id: u32,
    ) -> Vec<ClientEvent> {
        self.workspaces
            .get_mut(workspace_id)
            .and_then(|workspace| workspace.render_mailboxes.remove(&pane_id))
            .map(|events| events.into_iter().collect())
            .unwrap_or_default()
    }

    /// Take the newest queued authoritative baseline for one pane.
    ///
    /// Non-history events before that baseline are already represented by the
    /// snapshot and can be discarded. History and events after it remain
    /// queued for the render drain.
    pub fn take_pane_baseline(&mut self, workspace_id: &str, pane_id: u32) -> Option<Vec<u8>> {
        let data = {
            let mailbox = self
                .workspaces
                .get_mut(workspace_id)
                .and_then(|workspace| workspace.render_mailboxes.get_mut(&pane_id))?;
            let baseline_index = mailbox.iter().rposition(|event| {
                matches!(
                    event.kind(),
                    ClientEventKind::PaneSnapshot | ClientEventKind::PaneFrame
                )
            })?;
            let data = mailbox.get(baseline_index)?.data.clone();
            let mut retained = VecDeque::new();
            for _ in 0..=baseline_index {
                let event = mailbox
                    .pop_front()
                    .expect("baseline index must describe a queued event");
                if matches!(event.kind(), ClientEventKind::PaneHistory) {
                    retained.push_back(event);
                }
            }
            retained.extend(mailbox.drain(..));
            *mailbox = retained;
            data
        };
        if let Some(view) = self.workspaces.get_mut(workspace_id) {
            view.pending_baselines.remove(&pane_id);
        }
        Some(data)
    }

    /// Take a baseline requested when a paused pane becomes visible again.
    ///
    /// A resumed surface already has its last known pixels, so queued history
    /// is discarded after the baseline is found; output after that baseline
    /// remains queued for ordered catch-up.
    pub fn take_pane_resume_baseline(
        &mut self,
        workspace_id: &str,
        pane_id: u32,
    ) -> Option<Vec<u8>> {
        let pending = self
            .workspaces
            .get(workspace_id)
            .is_some_and(|view| view.pending_baselines.contains(&pane_id));
        if !pending {
            return None;
        }
        let data = self.take_pane_baseline(workspace_id, pane_id)?;
        if let Some(view) = self.workspaces.get_mut(workspace_id) {
            if let Some(mailbox) = view.render_mailboxes.get_mut(&pane_id) {
                mailbox.retain(|event| event.kind() != ClientEventKind::PaneHistory);
            }
        }
        Some(data)
    }
}

fn push_bounded(mailbox: &mut VecDeque<ClientEvent>, event: ClientEvent) {
    if mailbox.len() == RENDER_MAILBOX_CAPACITY {
        mailbox.pop_front();
    }
    mailbox.push_back(event);
}

fn push_coalesced(mailbox: &mut VecDeque<ClientEvent>, event: ClientEvent) {
    if event.kind() == ClientEventKind::PaneOutput {
        if let Some(previous) = mailbox.back_mut().filter(|previous| {
            previous.kind() == ClientEventKind::PaneOutput
                && previous.data.len() + event.data.len() <= COALESCED_OUTPUT_MAX_BYTES
        }) {
            previous.data.extend_from_slice(&event.data);
            return;
        }
    }
    push_bounded(mailbox, event);
}

fn push_paused(mailbox: &mut VecDeque<ClientEvent>, event: ClientEvent) {
    match event.kind() {
        ClientEventKind::PaneOutput => {}
        ClientEventKind::PaneSnapshot | ClientEventKind::PaneFrame => {
            let history = mailbox
                .drain(..)
                .filter(|queued| queued.kind() == ClientEventKind::PaneHistory)
                .collect::<VecDeque<_>>();
            *mailbox = history;
            mailbox.push_back(event);
        }
        ClientEventKind::PaneHistory => push_bounded(mailbox, event),
        ClientEventKind::PaneClosed | ClientEventKind::PaneResized | ClientEventKind::Other(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientEvent, ClientEventKind, ClientLayout, ClientPane, ClientTab, ClientWorkspace,
        PaneRenderPolicy, ViewStore, RENDER_MAILBOX_CAPACITY,
    };
    use muxterm_core::protocol::ffi::types;

    fn event_with_data(type_: u32, pane_id: u32, data: Vec<u8>) -> ClientEvent {
        ClientEvent {
            type_,
            pane_id,
            tab_id: 1,
            window_id: 0,
            data,
            name: String::new(),
        }
    }

    fn event(type_: u32, pane_id: u32, byte: u8) -> ClientEvent {
        event_with_data(type_, pane_id, vec![byte])
    }

    #[test]
    fn topology_is_owned_and_render_mailboxes_are_bounded() {
        let mut store = ViewStore::default();
        store.replace_topology(
            ClientWorkspace {
                id: "local//one/shell/".into(),
                name: "one".into(),
                runtime: "shell".into(),
                active: true,
                resolved_target: None,
            },
            vec![ClientTab {
                id: 1,
                name: "tab".into(),
                is_active: true,
            }],
            vec![(
                1,
                vec![ClientPane {
                    id: 7,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "bash".into(),
                }],
            )],
        );
        store.replace_layouts(
            "local//one/shell/",
            vec![(1, ClientLayout::Leaf { pane_id: 7 })],
        );
        for byte in 0..=u8::MAX {
            store.push_render_event(
                "local//one/shell/",
                event(types::STATE_PANE_OUTPUT, 7, byte),
            );
        }
        let view = store
            .workspace("local//one/shell/")
            .expect("workspace snapshot");
        assert_eq!(store.active_workspace_id(), Some("local//one/shell/"));
        assert_eq!(view.tabs[0].name, "tab");
        assert_eq!(view.panes[&1][0].title, "bash");
        assert_eq!(
            view.layouts.get(&1),
            Some(&ClientLayout::Leaf { pane_id: 7 })
        );

        let events = store.take_pane_render_events("local//one/shell/", 7);
        assert_eq!(events.len(), 128);
        assert_eq!(events.first().map(|event| event.data[0]), Some(128));
        assert_eq!(events.last().map(|event| event.data[0]), Some(255));
    }

    #[test]
    fn control_events_do_not_enter_render_mailboxes() {
        let mut store = ViewStore::default();
        store.push_render_event("local//one/shell/", event(types::STATE_TAB_ADDED, 0, 1));
        assert!(store
            .take_pane_render_events("local//one/shell/", 0)
            .is_empty());
    }

    #[test]
    fn chatty_panes_are_isolated_across_workspaces() {
        let mut store = ViewStore::default();
        let workspace_a = "local//one/shell/";
        let workspace_b = "ssh//two/tmux/";

        store.set_pane_render_policy(workspace_b, 9, PaneRenderPolicy::Pause);

        for byte in 0u8..=u8::MAX {
            store.push_render_event(workspace_a, event(types::STATE_PANE_OUTPUT, 7, byte));
            store.push_render_event(workspace_b, event(types::STATE_PANE_OUTPUT, 9, byte));
            if byte < 32 {
                store.push_render_event(workspace_a, event(types::STATE_PANE_OUTPUT, 8, byte));
                store.push_render_event(workspace_b, event(types::STATE_PANE_OUTPUT, 10, byte));
            }
        }

        let chatty = store.take_pane_render_events(workspace_a, 7);
        assert_eq!(chatty.len(), RENDER_MAILBOX_CAPACITY);
        assert_eq!(chatty.first().map(|event| event.data[0]), Some(128));
        assert_eq!(chatty.last().map(|event| event.data[0]), Some(u8::MAX));

        let workspace_a_quiet = store.take_pane_render_events(workspace_a, 8);
        assert_eq!(
            workspace_a_quiet
                .iter()
                .map(|event| event.data[0])
                .collect::<Vec<_>>(),
            (0u8..32u8).collect::<Vec<_>>()
        );

        let workspace_b_quiet = store.take_pane_render_events(workspace_b, 10);
        assert_eq!(
            workspace_b_quiet
                .iter()
                .map(|event| event.data[0])
                .collect::<Vec<_>>(),
            (0u8..32u8).collect::<Vec<_>>()
        );
        assert!(store.take_pane_render_events(workspace_b, 9).is_empty());
    }

    #[test]
    fn coalescing_is_per_pane_and_preserves_fifo_boundaries() {
        let mut store = ViewStore::default();
        let workspace_a = "local//one/shell/";
        let workspace_b = "ssh//two/tmux/";
        store.set_pane_render_policy(workspace_a, 7, PaneRenderPolicy::Coalesce);
        store.set_pane_render_policy(workspace_a, 8, PaneRenderPolicy::Coalesce);
        store.set_pane_render_policy(workspace_b, 9, PaneRenderPolicy::Coalesce);

        store.push_render_event(workspace_a, event(types::STATE_PANE_OUTPUT, 7, 1));
        store.push_render_event(workspace_b, event(types::STATE_PANE_OUTPUT, 9, 10));
        store.push_render_event(workspace_a, event(types::STATE_PANE_OUTPUT, 7, 2));
        store.push_render_event(workspace_a, event(types::STATE_PANE_OUTPUT, 8, 20));
        store.push_render_event(workspace_a, event(types::STATE_PANE_FRAME, 7, 3));
        store.push_render_event(workspace_a, event(types::STATE_PANE_OUTPUT, 7, 4));
        store.push_render_event(workspace_a, event(types::STATE_PANE_OUTPUT, 7, 5));
        store.push_render_event(workspace_b, event(types::STATE_PANE_OUTPUT, 9, 11));

        let pane_a = store.take_pane_render_events(workspace_a, 7);
        assert_eq!(pane_a.len(), 3);
        assert_eq!(pane_a[0].data, vec![1, 2]);
        assert_eq!(pane_a[1].kind(), ClientEventKind::PaneFrame);
        assert_eq!(pane_a[1].data, vec![3]);
        assert_eq!(pane_a[2].data, vec![4, 5]);

        let pane_a_other = store.take_pane_render_events(workspace_a, 8);
        assert_eq!(pane_a_other.len(), 1);
        assert_eq!(pane_a_other[0].data, vec![20]);

        let pane_b = store.take_pane_render_events(workspace_b, 9);
        assert_eq!(pane_b.len(), 1);
        assert_eq!(pane_b[0].data, vec![10, 11]);
    }

    #[test]
    fn newest_baseline_replaces_older_mailbox_data() {
        let mut store = ViewStore::default();
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_HISTORY, 7, 9));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_OUTPUT, 7, 1));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_SNAPSHOT, 7, 2));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_OUTPUT, 7, 3));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_FRAME, 7, 4));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_OUTPUT, 7, 5));

        assert_eq!(
            store.take_pane_baseline("local//one/shell/", 7),
            Some(vec![4])
        );
        let remaining = store.take_pane_render_events("local//one/shell/", 7);
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].kind(), ClientEventKind::PaneHistory);
        assert_eq!(remaining[0].data, vec![9]);
        assert_eq!(remaining[1].data, vec![5]);
    }

    #[test]
    fn coalesce_policy_merges_adjacent_output_without_losing_bytes() {
        let mut store = ViewStore::default();
        store.set_pane_render_policy("local//one/shell/", 7, PaneRenderPolicy::Coalesce);
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_OUTPUT, 7, 1));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_OUTPUT, 7, 2));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_FRAME, 7, 3));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_OUTPUT, 7, 4));

        let events = store.take_pane_render_events("local//one/shell/", 7);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].data, vec![1, 2]);
        assert_eq!(events[1].kind(), ClientEventKind::PaneFrame);
        assert_eq!(events[2].data, vec![4]);
    }

    #[test]
    fn pause_policy_drops_output_but_keeps_latest_baseline_and_history() {
        let mut store = ViewStore::default();
        store.set_pane_render_policy("local//one/shell/", 7, PaneRenderPolicy::Pause);
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_HISTORY, 7, 9));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_OUTPUT, 7, 1));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_SNAPSHOT, 7, 2));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_OUTPUT, 7, 3));
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_FRAME, 7, 4));

        assert_eq!(
            store.take_pane_baseline("local//one/shell/", 7),
            Some(vec![4])
        );
        let events = store.take_pane_render_events("local//one/shell/", 7);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), ClientEventKind::PaneHistory);
        assert_eq!(events[0].data, vec![9]);
    }

    #[test]
    fn resuming_paused_pane_waits_for_baseline_before_accepting_output() {
        let mut store = ViewStore::default();
        store.set_pane_render_policy("local//one/shell/", 7, PaneRenderPolicy::Pause);
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_SNAPSHOT, 7, 2));
        assert_eq!(
            store.set_pane_render_policy("local//one/shell/", 7, PaneRenderPolicy::Live),
            PaneRenderPolicy::Pause
        );
        store.push_render_event("local//one/shell/", event(types::STATE_PANE_OUTPUT, 7, 3));

        assert_eq!(
            store.take_pane_resume_baseline("local//one/shell/", 7),
            Some(vec![2])
        );
        assert!(store
            .take_pane_render_events("local//one/shell/", 7)
            .is_empty());
        assert_eq!(
            store.pane_render_policy("local//one/shell/", 7),
            PaneRenderPolicy::Live
        );
    }
}
