//! Cross-workspace activity domain.
//!
//! Attention is the first migrated activity projection. Commands, agents and
//! their lane events will join this owner as the Activity contract lands.

pub mod attention;
pub mod record;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::activity::attention::clock::RealClock;
use crate::activity::attention::engine::AttentionEngine;
use crate::config::AttentionConfig;
use muxterm_protocol::state::{PaneAgentInfo, PaneAgentStatus};
use muxterm_protocol::{ActivityId, PaneId, TabId, WorkspaceId};
use record::{ActivityEvent, ActivityKind, ActivityRecord, ActivityStatus, ActivityStore, PaneRef};

/// Product metadata needed to place one Runtime signal in the Activity lane.
///
/// Runtime only reports pane-local facts.  Workspace supplies this context so
/// Activity records never need to understand tmux/Herdr wire identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivityContext {
    pub workspace: WorkspaceId,
    pub pane: PaneId,
    pub tab: TabId,
    pub workspace_name: String,
    pub runtime_name: String,
    pub transport_name: String,
}

/// Activity owner for one Muxterm product session.
///
/// The attention projection remains available to the legacy FFI query surface;
/// new command and agent records share this owner and its revision watermark.
pub struct ActivityState {
    pub(crate) attention: AttentionEngine<RealClock>,
    pub(crate) records: ActivityStore,
}

impl ActivityState {
    pub(crate) fn new(config: AttentionConfig) -> Self {
        Self {
            attention: AttentionEngine::new(config, RealClock),
            records: ActivityStore::default(),
        }
    }

    /// Normalize one authoritative Runtime agent signal into a revisioned
    /// product event.  `None` releases the authority and removes the stable
    /// agent record for that pane.
    pub(crate) fn apply_agent_signal(
        &mut self,
        context: ActivityContext,
        agent: Option<&PaneAgentInfo>,
    ) -> ActivityEvent {
        let id = agent_activity_id(&context.workspace, context.pane);
        let event = match agent {
            Some(agent) => {
                let started_at = self.records.get(&id).and_then(|record| record.started_at);
                self.records.upsert(ActivityRecord {
                    id,
                    kind: ActivityKind::Agent,
                    name: agent_display_name(agent),
                    status: agent_status(agent.status),
                    location: PaneRef {
                        workspace: context.workspace,
                        tab: context.tab,
                        pane: context.pane,
                    },
                    cwd: agent_cwd(agent),
                    workspace_name: context.workspace_name,
                    runtime_name: context.runtime_name,
                    transport_name: context.transport_name,
                    started_at,
                    updated_at: unix_timestamp(),
                    revision: 0,
                })
            }
            None => self.records.remove(id),
        };
        event
    }

    pub(crate) fn agent_record(
        &self,
        workspace: &WorkspaceId,
        pane: PaneId,
    ) -> Option<&ActivityRecord> {
        let id = agent_activity_id(workspace, pane);
        self.records.get(&id)
    }

    pub(crate) fn remove_agent(
        &mut self,
        workspace: &WorkspaceId,
        pane: PaneId,
    ) -> Option<ActivityEvent> {
        let id = agent_activity_id(workspace, pane);
        self.records.get(&id)?;
        Some(self.records.remove(id))
    }
}

fn agent_activity_id(workspace: &WorkspaceId, pane: PaneId) -> ActivityId {
    ActivityId::new(format!("agent:{workspace}:{pane}")).expect("agent activity id is non-empty")
}

fn agent_display_name(agent: &PaneAgentInfo) -> String {
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
    .unwrap_or_else(|| "agent".into())
}

fn agent_status(status: PaneAgentStatus) -> ActivityStatus {
    match status {
        PaneAgentStatus::Working => ActivityStatus::Running,
        PaneAgentStatus::Done => ActivityStatus::Done { exit_code: None },
        PaneAgentStatus::Idle | PaneAgentStatus::Blocked | PaneAgentStatus::Unknown => {
            ActivityStatus::Waiting
        }
    }
}

fn agent_cwd(agent: &PaneAgentInfo) -> Option<PathBuf> {
    agent
        .cwd
        .as_deref()
        .or(agent.foreground_cwd.as_deref())
        .map(PathBuf::from)
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn context() -> ActivityContext {
        ActivityContext {
            workspace: WorkspaceId::new("local", None, "demo", "herdr", "/tmp/demo"),
            pane: PaneId(2),
            tab: TabId(3),
            workspace_name: "demo".into(),
            runtime_name: "herdr".into(),
            transport_name: "local".into(),
        }
    }

    fn agent(status: PaneAgentStatus) -> PaneAgentInfo {
        PaneAgentInfo {
            terminal_id: Some("terminal-1".into()),
            name: Some("codex".into()),
            kind: Some("coding".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_name: Some("Codex".into()),
            status,
            screen_detection_skipped: false,
            state_labels: BTreeMap::new(),
            tokens: BTreeMap::new(),
            session: None,
            focused: true,
            launch_pending: false,
            interactive_ready: true,
            state_change_seq: 4,
            cwd: Some("/tmp/demo".into()),
            foreground_cwd: None,
            revision: 8,
        }
    }

    #[test]
    fn agent_signal_creates_revisioned_product_record() {
        let mut state = ActivityState::new(AttentionConfig::default());
        let first = state.apply_agent_signal(context(), Some(&agent(PaneAgentStatus::Working)));
        let ActivityEvent::Upsert(record) = first else {
            panic!("agent signal must upsert");
        };
        assert_eq!(record.kind, ActivityKind::Agent);
        assert_eq!(record.name, "Codex");
        assert_eq!(record.status, ActivityStatus::Running);
        assert_eq!(record.location.tab, TabId(3));
        assert_eq!(record.revision, 1);
        assert!(record.updated_at > 0);
        assert_eq!(
            state.agent_record(&context().workspace, PaneId(2)),
            Some(record.as_ref())
        );
    }

    #[test]
    fn clearing_agent_authority_removes_the_same_record() {
        let mut state = ActivityState::new(AttentionConfig::default());
        state.apply_agent_signal(context(), Some(&agent(PaneAgentStatus::Working)));
        let removed = state.apply_agent_signal(context(), None);
        assert!(matches!(removed, ActivityEvent::Remove { revision: 2, .. }));
        assert!(state
            .agent_record(&context().workspace, PaneId(2))
            .is_none());
    }
}
