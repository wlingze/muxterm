//! 切 tab 等待确认的门禁（纯逻辑，便于单测）。
//!
//! 外部关闭 tab / 快照里已不存在 / 超时都会放行，避免 UI 一直等一个
//! 永远不会到达的 `STATE_ACTIVE_TAB_CHANGED`。

use std::time::{Duration, Instant};

/// 切 tab 门禁。
#[derive(Debug, Clone, Default)]
pub struct TabSwitchGate {
    pub timeout: Duration,
    pending_tab: Option<u32>,
    pending_since: Option<Instant>,
}

impl TabSwitchGate {
    pub fn new(timeout: Duration) -> Self {
        TabSwitchGate {
            timeout,
            pending_tab: None,
            pending_since: None,
        }
    }

    /// 发起一次切 tab：记住目标与时刻。
    pub fn request(&mut self, tab: u32) {
        self.pending_tab = Some(tab);
        self.pending_since = Some(Instant::now());
    }

    /// 收到激活 tab 变更且就是等待的目标：立即放行。
    pub fn on_tab_changed(&mut self, tab: u32) {
        if self.pending_tab == Some(tab) {
            self.clear();
        }
    }

    /// 等待中的 tab 被外部关闭：立即放行。
    pub fn on_tab_closed(&mut self, tab: u32) {
        if self.pending_tab == Some(tab) {
            self.clear();
        }
    }

    /// 快照更新：等待的目标已不存在 → 放行。
    pub fn on_snapshot(&mut self, tabs: &[u32]) {
        if let Some(pending) = self.pending_tab {
            if !tabs.contains(&pending) {
                self.clear();
            }
        }
    }

    /// 门禁是否放行：没有等待目标，或已超过超时。
    pub fn is_released(&self) -> bool {
        match self.pending_since {
            None => true,
            Some(since) => since.elapsed() > self.timeout,
        }
    }

    fn clear(&mut self) {
        self.pending_tab = None;
        self.pending_since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_when_nothing_pending() {
        let gate = TabSwitchGate::new(Duration::from_millis(1500));
        assert!(gate.is_released());
    }

    #[test]
    fn tab_changed_releases_immediately() {
        let mut gate = TabSwitchGate::new(Duration::from_secs(10));
        gate.request(3);
        assert!(!gate.is_released());
        gate.on_tab_changed(3);
        assert!(gate.is_released());
    }

    #[test]
    fn tab_closed_releases_immediately() {
        let mut gate = TabSwitchGate::new(Duration::from_secs(10));
        gate.request(5);
        gate.on_tab_closed(5);
        assert!(gate.is_released());
    }

    #[test]
    fn snapshot_missing_releases() {
        let mut gate = TabSwitchGate::new(Duration::from_secs(10));
        gate.request(9);
        gate.on_snapshot(&[1, 2]);
        assert!(gate.is_released());
    }
}
