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
use crate::protocol::state::{PaneAgentInfo, PaneAgentStatus};
use crate::protocol::{ActivityId, PaneId, TabId, WorkspaceId};
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

    /// Normalize an OSC 133 command-start signal into a running record.
    pub(crate) fn apply_command_start(
        &mut self,
        context: ActivityContext,
        name: Option<String>,
    ) -> ActivityEvent {
        let id = command_activity_id(&context.workspace, context.pane);
        let now = unix_timestamp();
        let fallback_name = self.records.get(&id).map(|record| record.name.clone());
        self.records.upsert(ActivityRecord {
            id,
            kind: ActivityKind::Command,
            name: name.or(fallback_name).unwrap_or_else(|| "command".into()),
            status: ActivityStatus::Running,
            location: PaneRef {
                workspace: context.workspace,
                tab: context.tab,
                pane: context.pane,
            },
            cwd: None,
            workspace_name: context.workspace_name,
            runtime_name: context.runtime_name,
            transport_name: context.transport_name,
            started_at: Some(now),
            updated_at: now,
            revision: 0,
        })
    }

    /// Normalize an OSC 133 command-done signal.  A non-zero exit code is a
    /// failed activity; an absent code remains a completed-but-unknown exit.
    pub(crate) fn apply_command_done(
        &mut self,
        context: ActivityContext,
        name: Option<String>,
        exit_code: Option<u8>,
    ) -> ActivityEvent {
        let id = command_activity_id(&context.workspace, context.pane);
        let existing = self.records.get(&id);
        let fallback_name = existing.map(|record| record.name.clone());
        let started_at = existing.and_then(|record| record.started_at);
        let now = unix_timestamp();
        let status = match exit_code {
            Some(code) if code != 0 => ActivityStatus::Failed,
            code => ActivityStatus::Done {
                exit_code: code.map(i32::from),
            },
        };
        self.records.upsert(ActivityRecord {
            id,
            kind: ActivityKind::Command,
            name: name.or(fallback_name).unwrap_or_else(|| "command".into()),
            status,
            location: PaneRef {
                workspace: context.workspace,
                tab: context.tab,
                pane: context.pane,
            },
            cwd: None,
            workspace_name: context.workspace_name,
            runtime_name: context.runtime_name,
            transport_name: context.transport_name,
            started_at,
            updated_at: now,
            revision: 0,
        })
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

fn command_activity_id(workspace: &WorkspaceId, pane: PaneId) -> ActivityId {
    ActivityId::new(format!("command:{workspace}:{pane}"))
        .expect("command activity id is non-empty")
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

    #[test]
    fn command_start_and_done_share_identity_and_revision() {
        let mut state = ActivityState::new(AttentionConfig::default());
        let started = state.apply_command_start(context(), Some("cargo test".into()));
        let ActivityEvent::Upsert(started) = started else {
            panic!("command start must upsert");
        };
        assert_eq!(started.kind, ActivityKind::Command);
        assert_eq!(started.status, ActivityStatus::Running);
        assert!(started.started_at.is_some());
        assert!(started.updated_at >= started.started_at.unwrap());

        let done = state.apply_command_done(context(), None, Some(0));
        let ActivityEvent::Upsert(done) = done else {
            panic!("command done must upsert");
        };
        assert_eq!(done.id, started.id);
        assert_eq!(done.name, "cargo test");
        assert_eq!(done.status, ActivityStatus::Done { exit_code: Some(0) });
        assert_eq!(done.revision, started.revision + 1);
        assert_eq!(done.started_at, started.started_at);
    }
}
