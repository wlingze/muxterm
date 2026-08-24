//! Herdr tab/pane mutation 队列：异步创建操作的权威收敛。
//!
//! `tab.create` / `pane.split` 的直接响应只提供「等待哪个 id」的线索，不是
//! 最终拓扑。本模块把 mutation 串行化（同时至多一个 in-flight），并用
//! snapshot/event 权威收敛 + 有界 probe 兜底，最终只发一次
//! `MutationSettled`（Completed 或带阶段的 Failed）。
//!
//! 时间全部显式注入 `Instant`，测试不需要真实 sleep。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::core::model::state::{MutationKind, MutationResult, MutationStage};

/// mutation FIFO 上限。
pub const MUTATION_QUEUE_MAX: usize = 32;
/// enqueue 起算的端到端 deadline。
pub const MUTATION_DEADLINE: Duration = Duration::from_secs(5);
/// 派发后 probe 的绝对时点（相对 dispatched_at）。
pub const PROBE_DELAYS_MS: [u64; 6] = [100, 250, 500, 1000, 2000, 4000];

/// 一个待收敛的异步 mutation。
#[derive(Debug, Clone)]
pub struct PendingMutation {
    pub mutation_id: u64,
    pub kind: MutationKind,
    /// NewTab 的显式名称（None = 完全省略 label）。
    pub new_tab_name: Option<String>,
    /// SplitPane 的方向。
    pub split_dir: Option<crate::core::model::layout::SplitDir>,
    /// 派发时的 lifecycle generation（detach 后晚到结果直接丢弃）。
    pub lifecycle_generation: u64,
    /// 派发时记录的目标（NewTab 无 target pane；SplitPane 有）。
    pub target_tab: Option<String>,
    pub target_pane: Option<String>,
    /// 派发时的拓扑 baseline（相对它找唯一新对象）。
    pub tabs_before: Option<HashSet<String>>,
    pub panes_before: Option<HashSet<String>>,
    /// 从响应/快照推导出的期望 id。
    pub expected_tab: Option<String>,
    pub expected_pane: Option<String>,
    pub expected_focus: Option<String>,
    pub enqueued_at: Instant,
    pub dispatched_at: Option<Instant>,
    pub next_probe_at: Option<Instant>,
    pub probe_index: usize,
    /// enqueue 起算的端到端 deadline。
    pub deadline: Instant,
}

impl PendingMutation {
    pub fn new(mutation_id: u64, kind: MutationKind, now: Instant) -> Self {
        Self {
            mutation_id,
            kind,
            new_tab_name: None,
            split_dir: None,
            lifecycle_generation: 0,
            target_tab: None,
            target_pane: None,
            tabs_before: None,
            panes_before: None,
            expected_tab: None,
            expected_pane: None,
            expected_focus: None,
            enqueued_at: now,
            dispatched_at: None,
            next_probe_at: None,
            probe_index: 0,
            deadline: now + MUTATION_DEADLINE,
        }
    }

    /// 是否已耗尽端到端 deadline。
    pub fn expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    /// 派发时记录 baseline 与 lifecycle generation（不能在 enqueue 时提前取）。
    pub fn mark_dispatched(
        &mut self,
        lifecycle_generation: u64,
        tabs: HashSet<String>,
        panes: HashSet<String>,
        now: Instant,
    ) {
        self.lifecycle_generation = lifecycle_generation;
        self.tabs_before = Some(tabs);
        self.panes_before = Some(panes);
        self.dispatched_at = Some(now);
        self.next_probe_at = Some(now + Duration::from_millis(PROBE_DELAYS_MS[0]));
        self.probe_index = 0;
    }

    /// 推进到下一个 probe 时点；返回 None 表示 probe 序列已耗尽。
    pub fn advance_probe(&mut self, _now: Instant) -> Option<Instant> {
        self.probe_index += 1;
        if self.probe_index >= PROBE_DELAYS_MS.len() {
            return None;
        }
        let at = self.dispatched_at? + Duration::from_millis(PROBE_DELAYS_MS[self.probe_index]);
        if at > self.deadline {
            return None;
        }
        self.next_probe_at = Some(at);
        Some(at)
    }

    /// 当前是否有一个 in-flight probe。
    pub fn probe_in_flight(&self, now: Instant) -> bool {
        self.next_probe_at.is_some_and(|at| now < at)
    }

    /// 相对派发时 baseline，snapshot 是否恰好出现一个符合 kind 的新对象。
    /// 多个候选保持 Pending（禁止任选）。
    pub fn unique_new_object(
        &self,
        current: &HashSet<String>,
        before: &Option<HashSet<String>>,
    ) -> Option<String> {
        let before = before.as_ref()?;
        let added: Vec<&String> = current.difference(before).collect();
        if added.len() == 1 {
            Some(added[0].clone())
        } else {
            None
        }
    }
}

/// 有界 mutation FIFO：同时至多一个 in-flight，其余排队。
#[derive(Debug, Default)]
pub struct MutationQueue {
    pub queue: Vec<PendingMutation>,
    pub next_id: u64,
}

/// mutation FIFO 满：入队被拒（Rejected，不能先 Accepted 再丢失）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationQueueFull;

impl MutationQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 入队；队列满返回 Err（Rejected，不能先 Accepted 再丢失）。
    pub fn enqueue(&mut self, kind: MutationKind, now: Instant) -> Result<u64, MutationQueueFull> {
        if self.queue.len() >= MUTATION_QUEUE_MAX {
            return Err(MutationQueueFull);
        }
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.queue.push(PendingMutation::new(id, kind, now));
        Ok(id)
    }

    /// 当前 in-flight 项（队列头，已派发）。
    pub fn in_flight(&self) -> Option<&PendingMutation> {
        self.queue.first().filter(|m| m.dispatched_at.is_some())
    }

    /// 队头是否已派发（同时至多一个 in-flight）。
    pub fn has_in_flight(&self) -> bool {
        self.in_flight().is_some()
    }

    /// 队头是否已派发（同时至多一个 in-flight）。
    pub fn has_in_flight_mut(&self) -> bool {
        self.in_flight().is_some()
    }

    /// 取队头（可变）。
    pub fn head_mut(&mut self) -> Option<&mut PendingMutation> {
        self.queue.first_mut()
    }

    /// 按 mutation id 取（可变）：入队后配置参数必须按 id 定位，
    /// 不能 head_mut() —— 前一个 mutation 仍在 in-flight 时队头不是刚入队的项，
    /// 会把新参数（含 expected 重置）写到旧项上，导致旧项永不收敛（agent e2e）。
    pub fn by_id_mut(&mut self, id: u64) -> Option<&mut PendingMutation> {
        self.queue.iter_mut().find(|m| m.mutation_id == id)
    }

    /// 完成/失败后弹出队头，返回下一项（若有）。
    pub fn pop_head(&mut self) -> Option<PendingMutation> {
        if self.queue.is_empty() {
            return None;
        }
        Some(self.queue.remove(0))
    }

    /// 队列里是否有等待派发的项。
    pub fn has_pending(&self) -> bool {
        self.queue.iter().any(|m| m.dispatched_at.is_none())
    }

    /// 队头等待派发的项（若有）。
    pub fn next_undispatched(&self) -> Option<&PendingMutation> {
        self.queue.iter().find(|m| m.dispatched_at.is_none())
    }

    /// 队头等待派发的项（可变）。
    pub fn next_undispatched_mut(&mut self) -> Option<&mut PendingMutation> {
        self.queue.iter_mut().find(|m| m.dispatched_at.is_none())
    }

    /// 队列深度（诊断）。
    pub fn depth(&self) -> usize {
        self.queue.len()
    }
}

/// 构造一次 Failed settlement（带阶段与原因）。
#[allow(clippy::unused_self)]
pub fn failed_settlement(
    _operation_id: u64,
    _kind: MutationKind,
    stage: MutationStage,
    reason: impl Into<String>,
) -> MutationResult {
    MutationResult::Failed {
        stage,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::SplitDir;

    fn now() -> Instant {
        Instant::now()
    }

    /// W9 回归：入队后按 id 配置参数（NewTab/SplitPane handler 必须用
    /// `by_id_mut`，不能用 `head_mut`）。前一个 mutation 仍在 in-flight 时
    /// 队头是旧项，head_mut 会把新参数（含 expected 重置）写到旧项上，
    /// 旧项永不收敛、5 秒 deadline 失败（agent e2e 可见）。
    #[test]
    fn by_id_mut_configures_just_enqueued_item_not_in_flight_head() {
        let mut q = MutationQueue::new();
        let t0 = now();
        let _first_id = q.enqueue(MutationKind::NewTab, t0).unwrap();
        let second_id = q.enqueue(MutationKind::SplitPane, t0).unwrap();
        // 第一项已派发（in-flight），队头是它。
        let tabs = HashSet::from(["w1:t1".to_string()]);
        let panes = HashSet::from(["w1:p1".to_string()]);
        q.head_mut().unwrap().mark_dispatched(1, tabs, panes, t0);
        q.head_mut().unwrap().expected_tab = Some("w1:t2".into());

        // 用 by_id_mut 配置刚入队的第二项：必须落在第二项，而不是队头。
        let pending = q.by_id_mut(second_id).expect("第二项存在");
        pending.target_tab = Some("w1:t2".into());
        pending.target_pane = Some("w1:p1".into());
        pending.split_dir = Some(SplitDir::Horizontal);
        pending.expected_tab = None;
        pending.expected_pane = None;

        assert_eq!(
            q.in_flight().unwrap().expected_tab.as_deref(),
            Some("w1:t2"),
            "in-flight 首项的 expected_tab 不得被后续入队配置清掉"
        );
        let second = q.by_id_mut(second_id).unwrap();
        assert_eq!(second.target_tab.as_deref(), Some("w1:t2"));
        assert_eq!(second.target_pane.as_deref(), Some("w1:p1"));
        assert_eq!(second.split_dir, Some(SplitDir::Horizontal));
        assert_eq!(second.expected_tab, None);
    }

    /// 入队返回递增 operation_id；队列满 Rejected。
    #[test]
    fn enqueue_returns_increasing_ids_and_bounds() {
        let mut q = MutationQueue::new();
        let t0 = now();
        let a = q.enqueue(MutationKind::NewTab, t0).unwrap();
        let b = q.enqueue(MutationKind::SplitPane, t0).unwrap();
        assert_eq!(a + 1, b);
        assert_eq!(q.depth(), 2);
        // 填满到上限。
        for _ in 2..MUTATION_QUEUE_MAX {
            q.enqueue(MutationKind::NewTab, t0).unwrap();
        }
        assert!(
            q.enqueue(MutationKind::NewTab, t0).is_err(),
            "队列满必须 Rejected"
        );
    }

    /// 同时至多一个 in-flight；队头派发后其余等待。
    #[test]
    fn only_one_in_flight_at_a_time() {
        let mut q = MutationQueue::new();
        let t0 = now();
        let id = q.enqueue(MutationKind::NewTab, t0).unwrap();
        q.enqueue(MutationKind::SplitPane, t0).unwrap();
        assert!(!q.has_in_flight());
        let tabs = HashSet::from(["w1:t1".to_string()]);
        let panes = HashSet::from(["w1:p1".to_string()]);
        q.head_mut().unwrap().mark_dispatched(1, tabs, panes, t0);
        assert!(q.has_in_flight());
        assert_eq!(q.in_flight().unwrap().mutation_id, id);
        // 队头完成前不能派发下一项。
        assert!(q.next_undispatched().is_some());
        q.pop_head();
        assert!(!q.has_in_flight());
        assert!(q.next_undispatched().is_some(), "第二项等待派发");
    }

    /// probe 序列：派发后 100/250/500/1000/2000/4000ms，且不越过 deadline。
    #[test]
    fn probe_schedule_is_absolute_after_dispatch() {
        let t0 = now();
        let mut m = PendingMutation::new(1, MutationKind::NewTab, t0);
        let tabs = HashSet::from(["w1:t1".to_string()]);
        let panes = HashSet::from(["w1:p1".to_string()]);
        m.mark_dispatched(1, tabs, panes, t0);
        assert_eq!(
            m.next_probe_at,
            Some(t0 + Duration::from_millis(PROBE_DELAYS_MS[0]))
        );
        for (i, expect_ms) in PROBE_DELAYS_MS.iter().enumerate().skip(1) {
            let at = m.advance_probe(t0).expect("probe 序列内");
            assert_eq!(
                at.duration_since(t0).as_millis() as u64,
                *expect_ms,
                "probe #{i} 应在 dispatched_at 后 {expect_ms}ms"
            );
        }
        // 越过 deadline 后不再安排。
        assert!(m.advance_probe(t0).is_none());
    }

    /// 端到端 deadline 从 enqueue 起算 5 秒。
    #[test]
    fn deadline_is_five_seconds_from_enqueue() {
        let t0 = now();
        let m = PendingMutation::new(1, MutationKind::NewTab, t0);
        assert!(!m.expired(t0 + Duration::from_secs(4)));
        assert!(m.expired(t0 + Duration::from_secs(5)));
    }

    /// 相对 baseline 恰好一个新增对象才填 expected id；多个候选保持 Pending。
    #[test]
    fn unique_new_object_requires_exactly_one_addition() {
        let t0 = now();
        let m = PendingMutation::new(1, MutationKind::NewTab, t0);
        let before = HashSet::from(["w1:t1".to_string()]);
        let current_one = HashSet::from(["w1:t1".to_string(), "w1:t2".to_string()]);
        assert_eq!(
            m.unique_new_object(&current_one, &Some(before.clone())),
            Some("w1:t2".to_string())
        );
        let current_two = HashSet::from([
            "w1:t1".to_string(),
            "w1:t2".to_string(),
            "w1:t3".to_string(),
        ]);
        assert_eq!(
            m.unique_new_object(&current_two, &Some(before)),
            None,
            "多个候选必须保持 Pending"
        );
        assert_eq!(m.unique_new_object(&current_one, &None), None);
    }
}
