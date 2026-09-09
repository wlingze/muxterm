//! Runtime event lanes.
//!
//! Runtime implementations may still use the legacy `StateChange` queue
//! internally while they migrate.  The public Runtime boundary is lane-based:
//! topology/control, render data, and runtime facts are kept separate before
//! Core turns them into product events.

use muxterm_protocol::layout::TabLayout;
use muxterm_protocol::state::{
    BackendStatus, MutationKind, MutationResult, PaneAgentInfo, StateChange,
};
use muxterm_protocol::{PaneId, TabId};
use serde::{Deserialize, Serialize};

/// One non-blocking drain from a Runtime instance.
///
/// The vectors preserve the Runtime's order within each lane.  Consumers must
/// apply them in product order: control, signals/activity, render baselines,
/// then incremental output.  `into_state_changes` provides that ordering for
/// compatibility callers that still consume the old mixed event enum.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeBatch {
    pub control: Vec<ControlEvent>,
    pub render: Vec<RenderEvent>,
    pub signals: Vec<RuntimeSignal>,
}

impl RuntimeBatch {
    /// Classify legacy product events into their lane at the Runtime boundary.
    pub fn from_state_changes(events: impl IntoIterator<Item = StateChange>) -> Self {
        let mut batch = Self::default();
        batch.extend_state_changes(events);
        batch
    }

    /// Add one legacy event to its lane.
    pub fn push_state_change(&mut self, event: StateChange) {
        match event {
            StateChange::PaneOutput { pane, data } => {
                self.render.push(RenderEvent::PaneOutput { pane, data });
            }
            StateChange::PaneSnapshot { pane, data } => {
                self.render.push(RenderEvent::PaneSnapshot { pane, data });
            }
            StateChange::PaneFrame { pane, data } => {
                self.render.push(RenderEvent::PaneFrame { pane, data });
            }
            StateChange::PaneIndexSnapshot { pane, data } => {
                self.render
                    .push(RenderEvent::PaneIndexSnapshot { pane, data });
            }
            StateChange::PaneHistory { pane, data } => {
                self.render.push(RenderEvent::PaneHistory { pane, data });
            }
            StateChange::PaneAgentChanged {
                pane,
                agent,
                initial,
            } => {
                self.signals.push(RuntimeSignal::PaneAgentChanged {
                    pane,
                    agent,
                    initial,
                });
            }
            StateChange::StatusBarSubscription { name, value, pane } => {
                self.signals
                    .push(RuntimeSignal::StatusBarSubscription { name, value, pane });
            }
            StateChange::TabAdded { tab } => self.control.push(ControlEvent::TabAdded { tab }),
            StateChange::TabClosed { tab } => self.control.push(ControlEvent::TabClosed { tab }),
            StateChange::TabRenamed { tab, name } => {
                self.control.push(ControlEvent::TabRenamed { tab, name });
            }
            StateChange::TabOrderChanged => self.control.push(ControlEvent::TabOrderChanged),
            StateChange::ActiveTabChanged { tab } => {
                self.control.push(ControlEvent::ActiveTabChanged { tab });
            }
            StateChange::LayoutChanged { tab, layout } => {
                self.control
                    .push(ControlEvent::LayoutChanged { tab, layout });
            }
            StateChange::PaneAdded { pane, tab } => {
                self.control.push(ControlEvent::PaneAdded { pane, tab });
            }
            StateChange::PaneClosed { pane } => {
                self.control.push(ControlEvent::PaneClosed { pane })
            }
            StateChange::PaneTitleChanged { pane, title } => {
                self.control
                    .push(ControlEvent::PaneTitleChanged { pane, title });
            }
            StateChange::PaneResized { pane, cols, rows } => {
                self.control
                    .push(ControlEvent::PaneResized { pane, cols, rows });
            }
            StateChange::ActivePaneChanged { tab, pane } => {
                self.control
                    .push(ControlEvent::ActivePaneChanged { tab, pane });
            }
            StateChange::WorkspaceRenamed { name } => {
                self.control.push(ControlEvent::WorkspaceRenamed { name });
            }
            StateChange::PoolChanged => self.control.push(ControlEvent::PoolChanged),
            StateChange::MutationSettled {
                operation_id,
                kind,
                result,
            } => self.control.push(ControlEvent::MutationSettled {
                operation_id,
                kind,
                result,
            }),
            StateChange::BackendStatusChanged(status) => {
                self.control
                    .push(ControlEvent::BackendStatusChanged(status));
            }
        }
    }

    /// Append legacy events while retaining lane-local order.
    pub fn extend_state_changes(&mut self, events: impl IntoIterator<Item = StateChange>) {
        for event in events {
            self.push_state_change(event);
        }
    }

    /// Append another batch to this batch.
    pub fn append(&mut self, other: Self) {
        self.control.extend(other.control);
        self.render.extend(other.render);
        self.signals.extend(other.signals);
    }

    pub fn is_empty(&self) -> bool {
        self.control.is_empty() && self.render.is_empty() && self.signals.is_empty()
    }

    /// Return the compatibility event stream in product delivery order.
    pub fn into_state_changes(self) -> Vec<StateChange> {
        let mut events =
            Vec::with_capacity(self.control.len() + self.render.len() + self.signals.len());
        events.extend(
            self.control
                .into_iter()
                .map(ControlEvent::into_state_change),
        );
        events.extend(
            self.signals
                .into_iter()
                .map(RuntimeSignal::into_state_change),
        );

        // A baseline must reach the consumer before incremental output.  This
        // is the batch-level fence that lets a Surface install a frame or
        // history without replaying a later output prefix first.
        let mut baselines = Vec::new();
        let mut output = Vec::new();
        for event in self.render {
            if matches!(event, RenderEvent::PaneOutput { .. }) {
                output.push(event);
            } else {
                baselines.push(event);
            }
        }
        events.extend(baselines.into_iter().map(RenderEvent::into_state_change));
        events.extend(output.into_iter().map(RenderEvent::into_state_change));
        events
    }
}

/// Topology, layout, focus, status and mutation events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlEvent {
    TabAdded {
        tab: TabId,
    },
    TabClosed {
        tab: TabId,
    },
    TabRenamed {
        tab: TabId,
        name: String,
    },
    TabOrderChanged,
    ActiveTabChanged {
        tab: TabId,
    },
    LayoutChanged {
        tab: TabId,
        layout: TabLayout,
    },
    PaneAdded {
        pane: PaneId,
        tab: TabId,
    },
    PaneClosed {
        pane: PaneId,
    },
    PaneTitleChanged {
        pane: PaneId,
        title: String,
    },
    PaneResized {
        pane: PaneId,
        cols: u16,
        rows: u16,
    },
    ActivePaneChanged {
        tab: TabId,
        pane: PaneId,
    },
    WorkspaceRenamed {
        name: String,
    },
    PoolChanged,
    MutationSettled {
        operation_id: u64,
        kind: MutationKind,
        result: MutationResult,
    },
    BackendStatusChanged(BackendStatus),
}

impl ControlEvent {
    fn into_state_change(self) -> StateChange {
        match self {
            Self::TabAdded { tab } => StateChange::TabAdded { tab },
            Self::TabClosed { tab } => StateChange::TabClosed { tab },
            Self::TabRenamed { tab, name } => StateChange::TabRenamed { tab, name },
            Self::TabOrderChanged => StateChange::TabOrderChanged,
            Self::ActiveTabChanged { tab } => StateChange::ActiveTabChanged { tab },
            Self::LayoutChanged { tab, layout } => StateChange::LayoutChanged { tab, layout },
            Self::PaneAdded { pane, tab } => StateChange::PaneAdded { pane, tab },
            Self::PaneClosed { pane } => StateChange::PaneClosed { pane },
            Self::PaneTitleChanged { pane, title } => StateChange::PaneTitleChanged { pane, title },
            Self::PaneResized { pane, cols, rows } => StateChange::PaneResized { pane, cols, rows },
            Self::ActivePaneChanged { tab, pane } => StateChange::ActivePaneChanged { tab, pane },
            Self::WorkspaceRenamed { name } => StateChange::WorkspaceRenamed { name },
            Self::PoolChanged => StateChange::PoolChanged,
            Self::MutationSettled {
                operation_id,
                kind,
                result,
            } => StateChange::MutationSettled {
                operation_id,
                kind,
                result,
            },
            Self::BackendStatusChanged(status) => StateChange::BackendStatusChanged(status),
        }
    }
}

/// Raw render data.  Runtime owns wire decoding; these bytes are already
/// suitable for Index and Surface consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderEvent {
    PaneOutput { pane: PaneId, data: Vec<u8> },
    PaneSnapshot { pane: PaneId, data: Vec<u8> },
    PaneFrame { pane: PaneId, data: Vec<u8> },
    PaneIndexSnapshot { pane: PaneId, data: Vec<u8> },
    PaneHistory { pane: PaneId, data: Vec<u8> },
}

impl RenderEvent {
    fn into_state_change(self) -> StateChange {
        match self {
            Self::PaneOutput { pane, data } => StateChange::PaneOutput { pane, data },
            Self::PaneSnapshot { pane, data } => StateChange::PaneSnapshot { pane, data },
            Self::PaneFrame { pane, data } => StateChange::PaneFrame { pane, data },
            Self::PaneIndexSnapshot { pane, data } => StateChange::PaneIndexSnapshot { pane, data },
            Self::PaneHistory { pane, data } => StateChange::PaneHistory { pane, data },
        }
    }
}

/// Runtime facts consumed by Core's Activity/attention normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeSignal {
    PaneAgentChanged {
        pane: PaneId,
        agent: Option<Box<PaneAgentInfo>>,
        initial: bool,
    },
    StatusBarSubscription {
        name: String,
        value: String,
        pane: Option<PaneId>,
    },
}

impl RuntimeSignal {
    fn into_state_change(self) -> StateChange {
        match self {
            Self::PaneAgentChanged {
                pane,
                agent,
                initial,
            } => StateChange::PaneAgentChanged {
                pane,
                agent,
                initial,
            },
            Self::StatusBarSubscription { name, value, pane } => {
                StateChange::StatusBarSubscription { name, value, pane }
            }
        }
    }
}

impl From<StateChange> for RuntimeBatch {
    fn from(event: StateChange) -> Self {
        Self::from_state_changes([event])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_events_and_delivers_baseline_before_output() {
        let events = vec![
            StateChange::PaneOutput {
                pane: PaneId(7),
                data: vec![0xff, 0x00],
            },
            StateChange::PaneFrame {
                pane: PaneId(7),
                data: b"frame".to_vec(),
            },
            StateChange::StatusBarSubscription {
                name: "muxterm.pane-cmd".into(),
                value: "cargo test".into(),
                pane: Some(PaneId(7)),
            },
            StateChange::TabOrderChanged,
        ];

        let batch = RuntimeBatch::from_state_changes(events);
        assert_eq!(batch.control, vec![ControlEvent::TabOrderChanged]);
        assert_eq!(batch.signals.len(), 1);
        assert_eq!(batch.render.len(), 2);

        let ordered = batch.into_state_changes();
        assert!(matches!(ordered[0], StateChange::TabOrderChanged));
        assert!(matches!(
            ordered[1],
            StateChange::StatusBarSubscription { .. }
        ));
        assert!(matches!(ordered[2], StateChange::PaneFrame { .. }));
        assert!(matches!(ordered[3], StateChange::PaneOutput { .. }));
        assert_eq!(
            ordered[3],
            StateChange::PaneOutput {
                pane: PaneId(7),
                data: vec![0xff, 0x00],
            }
        );
    }

    #[test]
    fn append_keeps_each_lane_fifo() {
        let mut first = RuntimeBatch::from(StateChange::PaneOutput {
            pane: PaneId(1),
            data: b"one".to_vec(),
        });
        first.append(RuntimeBatch::from(StateChange::PaneOutput {
            pane: PaneId(2),
            data: b"two".to_vec(),
        }));

        assert_eq!(
            first.render,
            vec![
                RenderEvent::PaneOutput {
                    pane: PaneId(1),
                    data: b"one".to_vec(),
                },
                RenderEvent::PaneOutput {
                    pane: PaneId(2),
                    data: b"two".to_vec(),
                },
            ]
        );
    }
}
