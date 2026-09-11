//! Activity lane records and revision-aware aggregation.
//!
//! Runtime adapters report facts; this module owns the product-facing
//! `Upsert`/`Remove` contract and makes out-of-order delivery self-healing.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::protocol::{ActivityId, PaneId, TabId, WorkspaceId};
use serde::{Deserialize, Serialize};

/// The product-level source represented by an activity record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Command,
    Agent,
}

/// Lifecycle status shared by command and agent projections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ActivityStatus {
    Running,
    Waiting,
    Done { exit_code: Option<i32> },
    Failed,
}

/// Product location used by frontends to activate the owning pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRef {
    pub workspace: WorkspaceId,
    pub tab: TabId,
    pub pane: PaneId,
}

/// Complete state for one command or agent activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRecord {
    pub id: ActivityId,
    pub kind: ActivityKind,
    pub name: String,
    pub status: ActivityStatus,
    pub location: PaneRef,
    pub cwd: Option<PathBuf>,
    pub workspace_name: String,
    pub runtime_name: String,
    pub transport_name: String,
    pub started_at: Option<u64>,
    pub updated_at: u64,
    pub revision: u64,
}

/// Idempotent activity lane event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityEvent {
    Upsert(Box<ActivityRecord>),
    Remove { id: ActivityId, revision: u64 },
}

impl ActivityEvent {
    pub fn id(&self) -> &ActivityId {
        match self {
            Self::Upsert(record) => &record.id,
            Self::Remove { id, .. } => id,
        }
    }

    pub fn revision(&self) -> u64 {
        match self {
            Self::Upsert(record) => record.revision,
            Self::Remove { revision, .. } => *revision,
        }
    }
}

/// Cross-workspace activity map with monotonic local revisions.
#[derive(Debug, Default)]
pub struct ActivityStore {
    records: BTreeMap<ActivityId, ActivityRecord>,
    tombstones: BTreeMap<ActivityId, u64>,
    next_revision: u64,
}

impl ActivityStore {
    pub fn records(&self) -> impl Iterator<Item = &ActivityRecord> {
        self.records.values()
    }

    pub fn get(&self, id: &ActivityId) -> Option<&ActivityRecord> {
        self.records.get(id)
    }

    /// Assign a fresh revision, store the complete record and return its event.
    pub fn upsert(&mut self, mut record: ActivityRecord) -> ActivityEvent {
        record.revision = self.fresh_revision();
        let event = ActivityEvent::Upsert(Box::new(record.clone()));
        self.tombstones.remove(&record.id);
        self.records.insert(record.id.clone(), record);
        event
    }

    /// Remove a record and return the tombstone event for downstream consumers.
    pub fn remove(&mut self, id: ActivityId) -> ActivityEvent {
        let revision = self.fresh_revision();
        self.records.remove(&id);
        self.tombstones.insert(id.clone(), revision);
        ActivityEvent::Remove { id, revision }
    }

    /// Apply a possibly delayed event. Older revisions never overwrite newer
    /// state; a newer remove also advances the local revision watermark.
    pub fn apply(&mut self, event: ActivityEvent) -> bool {
        let id = event.id().clone();
        let revision = event.revision();
        let current_revision = self
            .records
            .get(&id)
            .map(|record| record.revision)
            .into_iter()
            .chain(self.tombstones.get(&id).copied())
            .max();
        if current_revision.is_some_and(|current| revision <= current) {
            return false;
        }
        self.next_revision = self.next_revision.max(revision);
        match event {
            ActivityEvent::Upsert(record) => {
                self.tombstones.remove(&id);
                self.records.insert(id, *record);
            }
            ActivityEvent::Remove { .. } => {
                self.records.remove(&id);
                self.tombstones.insert(id, revision);
            }
        }
        true
    }

    fn fresh_revision(&mut self) -> u64 {
        self.next_revision = self.next_revision.saturating_add(1).max(1);
        self.next_revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, revision: u64) -> ActivityRecord {
        ActivityRecord {
            id: ActivityId::new(id).unwrap(),
            kind: ActivityKind::Command,
            name: "cargo test".into(),
            status: ActivityStatus::Running,
            location: PaneRef {
                workspace: WorkspaceId::new("local", None, "demo", "shell", "/tmp/demo"),
                tab: TabId(1),
                pane: PaneId(2),
            },
            cwd: Some("/tmp/demo".into()),
            workspace_name: "demo".into(),
            runtime_name: "shell".into(),
            transport_name: "local".into(),
            started_at: Some(10),
            updated_at: 11,
            revision,
        }
    }

    #[test]
    fn store_assigns_monotonic_revisions_and_removes_records() {
        let mut store = ActivityStore::default();
        let first = store.upsert(record("command-1", 0));
        let second = store.upsert(record("command-2", 0));

        assert_eq!(first.revision(), 1);
        assert_eq!(second.revision(), 2);
        assert_eq!(store.records().count(), 2);

        let removed = store.remove(ActivityId::new("command-1").unwrap());
        assert_eq!(removed.revision(), 3);
        assert!(store.get(removed.id()).is_none());
    }

    #[test]
    fn stale_upsert_and_remove_cannot_resurrect_or_overwrite_state() {
        let mut store = ActivityStore::default();
        let fresh = ActivityEvent::Upsert(Box::new(record("command-1", 8)));
        assert!(store.apply(fresh));

        let stale = ActivityEvent::Upsert(Box::new(record("command-1", 7)));
        assert!(!store.apply(stale));
        assert_eq!(
            store
                .get(&ActivityId::new("command-1").unwrap())
                .unwrap()
                .revision,
            8
        );

        let remove = ActivityEvent::Remove {
            id: ActivityId::new("command-1").unwrap(),
            revision: 9,
        };
        assert!(store.apply(remove));
        let stale_restore = ActivityEvent::Upsert(Box::new(record("command-1", 8)));
        assert!(!store.apply(stale_restore));
        assert!(store.get(&ActivityId::new("command-1").unwrap()).is_none());
    }

    #[test]
    fn activity_events_round_trip_as_frontend_data() {
        let event = ActivityEvent::Upsert(Box::new(record("agent-1", 4)));
        let json = serde_json::to_value(&event).unwrap();
        let restored: ActivityEvent = serde_json::from_value(json).unwrap();
        assert_eq!(restored, event);
    }
}
