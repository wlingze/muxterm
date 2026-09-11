//! 尚未 attach 的 target / session 台账（探活、灯）。
//!
//! 与 Pool 里已打开格子的 `BackendStatus` 不是一层。

use std::collections::HashMap;

/// 探活结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Unknown,
    Ok,
    Err,
}

/// Inventory 里一行：一个 (transport, target)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryEntry {
    pub transport_id: String,
    pub target: String,
    pub reach: Reach,
}

/// UI 只读的快照。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventorySnapshot {
    pub entries: Vec<InventoryEntry>,
}

impl InventorySnapshot {
    pub fn reach(&self, transport_id: &str, target: &str) -> Option<Reach> {
        self.entries
            .iter()
            .find(|e| e.transport_id == transport_id && e.target == target)
            .map(|e| e.reach)
    }
}

/// 未打开对象的探活缓存。
#[derive(Debug, Default)]
pub struct Inventory {
    entries: HashMap<(String, String), Reach>,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录探活结果。不打开 Workspace。
    pub fn mark(
        &mut self,
        transport_id: impl Into<String>,
        target: impl Into<String>,
        reach: Reach,
    ) {
        self.entries
            .insert((transport_id.into(), target.into()), reach);
    }

    pub fn snapshot(&self) -> InventorySnapshot {
        let mut entries: Vec<InventoryEntry> = self
            .entries
            .iter()
            .map(|((transport_id, target), reach)| InventoryEntry {
                transport_id: transport_id.clone(),
                target: target.clone(),
                reach: *reach,
            })
            .collect();
        entries.sort_by(|a, b| {
            a.transport_id
                .cmp(&b.transport_id)
                .then(a.target.cmp(&b.target))
        });
        InventorySnapshot { entries }
    }
}
