//! Muxterm product composition root.
//!
//! `Muxterm` owns the cross-domain state needed by one product session.  The
//! C ABI keeps a raw pointer to this value for compatibility, but the handle
//! is no longer the owner of a single runtime: it owns Catalog, Projects,
//! WorkspacePool and Activity-facing attention state together.

use std::collections::VecDeque;
use std::ffi::{c_char, CString};
use std::ptr;

use crate::core::attention::clock::RealClock;
use crate::core::attention::engine::AttentionEngine;
use crate::core::attention::signal::AttentionSignal;
use crate::core::config_service::SettingsService;
use crate::core::projects::ProjectsService;
use crate::core::protocol::state::StateChange;
use crate::core::workspace::pool::WorkspacePool;
use crate::core::workspace::workspace::Workspace;

use crate::core::protocol::ffi::callbacks::FfiCallbacks;
use crate::core::protocol::ffi::types::CLayoutNode;

type PendingAttentionUpdate = (u32, Vec<AttentionSignal>, String, u64, Option<String>);

/// One product session composed from Core domains.
///
/// The fields remain crate-visible while the FFI function modules are being
/// migrated to service methods.  Frontends never receive this Rust object;
/// they use the public FFI facade and its safe client wrapper.
pub struct Muxterm {
    /// Catalog and its live WorkspacePool during the compatibility migration.
    pub(crate) catalog: crate::core::catalog::Catalog,
    /// Core-owned Project records projected from the same SettingsService.
    pub(crate) projects: ProjectsService,
    /// Synchronous executor used by the C ABI boundary.
    pub(crate) rt: tokio::runtime::Runtime,
    pub(crate) callbacks: FfiCallbacks,
    /// Cross-workspace attention aggregation; Activity will own this later.
    pub(crate) attention: AttentionEngine<RealClock>,
    /// Core-owned configuration service shared by adapters.
    pub(crate) settings: SettingsService,
    /// C buffers whose pointers remain valid until the next poll/query.
    pub(crate) event_data: Vec<Vec<u8>>,
    pub(crate) event_names: Vec<CString>,
    pub(crate) tab_names: Vec<CString>,
    pub(crate) layout_nodes: Vec<CLayoutNode>,
    pub(crate) deferred_events: VecDeque<(crate::core::workspace::id::WorkspaceId, StateChange)>,
    pub(crate) workspace_ids: Vec<CString>,
}

impl Muxterm {
    pub(crate) fn pool(&self) -> &WorkspacePool {
        self.catalog.pool()
    }

    pub(crate) fn pool_mut(&mut self) -> &mut WorkspacePool {
        self.catalog.pool_mut()
    }

    pub(crate) fn projects(&self) -> &ProjectsService {
        &self.projects
    }

    pub(crate) fn projects_mut(&mut self) -> &mut ProjectsService {
        &mut self.projects
    }

    pub(crate) fn active_workspace(&self) -> Option<&Workspace> {
        self.pool().active()
    }

    pub(crate) fn active_workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.pool_mut().active_mut()
    }

    pub(crate) fn clear_event_bufs(&mut self) {
        self.event_data.clear();
        self.event_names.clear();
    }

    pub(crate) fn push_name(&mut self, value: &str) -> *const c_char {
        match CString::new(value) {
            Ok(value) => {
                self.event_names.push(value);
                self.event_names.last().unwrap().as_ptr()
            }
            Err(_) => ptr::null(),
        }
    }

    pub(crate) fn push_data(&mut self, data: &[u8]) -> (*const u8, usize) {
        self.event_data.push(data.to_vec());
        let value = self.event_data.last().unwrap();
        (value.as_ptr(), value.len())
    }

    pub(crate) fn push_workspace_id(&mut self, value: &str) -> *const c_char {
        match CString::new(value) {
            Ok(value) => {
                self.workspace_ids.push(value);
                self.workspace_ids.last().unwrap().as_ptr()
            }
            Err(_) => ptr::null(),
        }
    }

    pub(crate) fn defer_event(
        &mut self,
        workspace_id: crate::core::workspace::id::WorkspaceId,
        event: StateChange,
    ) {
        if should_export_state_change(&event) {
            self.deferred_events.push_back((workspace_id, event));
        }
    }

    /// Apply runtime signals to the cross-workspace attention projection.
    pub(crate) fn apply_attention_for_events(
        &mut self,
        ws_id: &crate::core::workspace::id::WorkspaceId,
        events: &[StateChange],
    ) {
        let mut pending: Vec<PendingAttentionUpdate> = Vec::new();
        let mut pending_process_names: Vec<(u32, Option<String>, bool)> = Vec::new();
        let mut removed_panes = Vec::new();
        {
            let Some(ws) = self.pool_mut().get_mut(ws_id) else {
                return;
            };
            for event in events {
                if let StateChange::PaneOutput { pane, .. }
                | StateChange::PaneSnapshot { pane, .. }
                | StateChange::PaneFrame { pane, .. }
                | StateChange::PaneIndexSnapshot { pane, .. }
                | StateChange::PaneHistory { pane, .. }
                | StateChange::PaneAgentChanged { pane, .. } = event
                {
                    if let StateChange::PaneAgentChanged { agent, .. } = event {
                        let process_name = agent.as_deref().and_then(|agent| {
                            [
                                agent.display_name.as_deref(),
                                agent.title.as_deref(),
                                agent.name.as_deref(),
                                agent.kind.as_deref(),
                            ]
                            .into_iter()
                            .flatten()
                            .find(|value| !value.trim().is_empty())
                            .map(str::to_string)
                        });
                        pending_process_names.push((pane.0, process_name, true));
                    }
                    let signals = ws.take_attention_signals(*pane);
                    let (last_line, seq) = ws.pane_last_line_seq(*pane);
                    let command = signals
                        .iter()
                        .any(|signal| matches!(signal, AttentionSignal::CommandStart))
                        .then(|| {
                            ws.pane_command_marks(*pane)
                                .last()
                                .map(|mark| mark.command.clone())
                        })
                        .flatten();
                    pending.push((pane.0, signals, last_line, seq, command));
                } else if let StateChange::PaneClosed { pane } = event {
                    removed_panes.push(pane.0);
                } else if let StateChange::StatusBarSubscription {
                    name,
                    value,
                    pane: Some(pane),
                } = event
                {
                    if name.starts_with("muxterm.pane-cmd") {
                        pending_process_names.push((
                            pane.0,
                            (!value.is_empty()).then(|| value.clone()),
                            false,
                        ));
                    }
                }
            }
        }
        let ws_name = ws_id.replica_id();
        for (pane, name, is_agent) in pending_process_names {
            if is_agent {
                self.attention.set_agent_process_name(&ws_name, pane, name);
            } else {
                self.attention.set_process_name(&ws_name, pane, name);
            }
        }
        for (pane, signals, last_line, seq, command) in pending {
            if let Some(command) = command {
                self.attention
                    .set_process_name(&ws_name, pane, Some(command));
            }
            self.attention
                .apply(&ws_name, pane, &signals, &last_line, seq);
        }
        for pane in removed_panes {
            self.attention.remove_pane(&ws_name, pane);
        }
    }
}

pub(crate) fn should_export_state_change(event: &StateChange) -> bool {
    !matches!(event, StateChange::PaneIndexSnapshot { .. })
}
