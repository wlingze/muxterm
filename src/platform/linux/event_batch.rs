//! Ordered Core event batches for the Linux frontend.
//!
//! The three-lane ordering is a frontend contract: topology must be realized
//! before a frame/history baseline, and incremental output is applied last.
//! Keeping this pure function outside `window.rs` makes the ordering reusable
//! by the eventual FFI `EventPump` and independently testable.

use crate::core::protocol::state::StateChange;

/// Return event indices in topology, baseline, and output order.
pub(crate) fn batch_order_plan(events: &[StateChange]) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let has_structural = events.iter().any(|event| {
        matches!(
            event,
            StateChange::TabAdded { .. }
                | StateChange::TabClosed { .. }
                | StateChange::LayoutChanged { .. }
                | StateChange::PaneAdded { .. }
                | StateChange::PaneClosed { .. }
                | StateChange::PaneResized { .. }
        )
    });
    if !has_structural {
        return ((0..events.len()).collect(), Vec::new(), Vec::new());
    }

    let mut structure = Vec::new();
    let mut baseline = Vec::new();
    let mut output = Vec::new();
    for (index, event) in events.iter().enumerate() {
        match event {
            StateChange::PaneSnapshot { .. }
            | StateChange::PaneFrame { .. }
            | StateChange::PaneHistory { .. } => baseline.push(index),
            StateChange::PaneOutput { .. } => output.push(index),
            _ => structure.push(index),
        }
    }
    (structure, baseline, output)
}
