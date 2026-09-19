//! 首次 attach 的历史先于 Surface baseline 发布；不暂停传输、不修改 live 字节。
//! 历史晚到时不能在前端已开始处理的 CSI/UTF-8 中插入清屏与回填序列。

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::protocol::PaneId;
use crate::runtime::{RenderEvent, RuntimeBatch};

const MAX_WAIT: Duration = Duration::from_secs(2);
const MAX_BYTES: usize = 8 * 1024 * 1024;

struct Pending {
    started: Instant,
    bytes: usize,
    history: bool,
    events: Vec<RenderEvent>,
}

#[derive(Default)]
pub(super) struct InitialHistory {
    pending: HashMap<PaneId, Pending>,
}

impl InitialHistory {
    /// 超时/超量时优先交出原始流，并要求调用方忽略随后到达的历史。
    pub(super) fn project(
        &mut self,
        batch: &mut RuntimeBatch,
        waiting: &HashSet<PaneId>,
        now: Instant,
    ) -> Vec<PaneId> {
        let ready: HashSet<_> = batch
            .render
            .iter()
            .filter_map(|event| match event {
                RenderEvent::PaneHistory { pane, .. } => Some(*pane),
                _ => None,
            })
            .collect();
        for event in std::mem::take(&mut batch.render) {
            let (pane, bytes) = match &event {
                RenderEvent::PaneSnapshot { pane, data }
                | RenderEvent::PaneFrame { pane, data }
                | RenderEvent::PaneOutput { pane, data }
                | RenderEvent::PaneHistory { pane, data } => (*pane, data.len()),
                RenderEvent::PaneIndexSnapshot { .. } => {
                    batch.render.push(event);
                    continue;
                }
            };
            if !waiting.contains(&pane)
                && !ready.contains(&pane)
                && !self.pending.contains_key(&pane)
            {
                batch.render.push(event);
                continue;
            }
            let pending = self.pending.entry(pane).or_insert_with(|| Pending {
                started: now,
                bytes: 0,
                history: false,
                events: Vec::new(),
            });
            pending.bytes += bytes;
            if matches!(event, RenderEvent::PaneHistory { .. }) {
                pending.history = true;
                pending.events.insert(0, event);
            } else {
                pending.events.push(event);
            }
        }
        let mut expired = Vec::new();
        self.pending.retain(|pane, pending| {
            let bounded =
                now.duration_since(pending.started) >= MAX_WAIT || pending.bytes >= MAX_BYTES;
            if pending.history || !waiting.contains(pane) || bounded {
                if bounded && !pending.history && waiting.contains(pane) {
                    expired.push(*pane);
                }
                batch.render.append(&mut pending.events);
                false
            } else {
                true
            }
        });
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_history_precedes_snapshot_and_unmodified_partial_output() {
        let mut gate = InitialHistory::default();
        let pane = PaneId(1);
        let now = Instant::now();
        let waiting = HashSet::from([pane]);
        let snapshot = RenderEvent::PaneSnapshot {
            pane,
            data: b"SCREEN\x1b[10;".to_vec(),
        };
        let output = RenderEvent::PaneOutput {
            pane,
            data: b"3HINPUT".to_vec(),
        };
        let history = RenderEvent::PaneHistory {
            pane,
            data: b"older\n".to_vec(),
        };
        let mut batch = RuntimeBatch {
            render: vec![snapshot.clone(), output.clone()],
            ..Default::default()
        };
        gate.project(&mut batch, &waiting, now);
        assert!(batch.render.is_empty());
        batch.render.push(history.clone());
        gate.project(&mut batch, &HashSet::new(), now);
        assert_eq!(batch.render, vec![history, snapshot, output]);
    }

    #[test]
    fn timeout_releases_bytes_without_waiting_forever_or_blocking_other_panes() {
        let mut gate = InitialHistory::default();
        let pane = PaneId(1);
        let waiting = HashSet::from([pane]);
        let now = Instant::now();
        let snapshot = RenderEvent::PaneSnapshot {
            pane,
            data: b"SCREEN".to_vec(),
        };
        let other = RenderEvent::PaneOutput {
            pane: PaneId(2),
            data: b"OTHER".to_vec(),
        };
        let mut batch = RuntimeBatch {
            render: vec![snapshot.clone(), other.clone()],
            ..Default::default()
        };
        gate.project(&mut batch, &waiting, now);
        assert_eq!(batch.render, vec![other]);
        batch.render.clear();
        assert_eq!(
            gate.project(&mut batch, &waiting, now + MAX_WAIT),
            vec![pane]
        );
        assert_eq!(batch.render, vec![snapshot]);
    }
}
