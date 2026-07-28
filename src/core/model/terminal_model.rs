//! TerminalModel：Terminal 层的纯逻辑核心。
//!
//! 持有一个 [`Backend`]，提供：
//! - `execute(Task)`：把任务派发给 backend，并聚合产生的 `StateChange` 事件
//! - `current_state()`：只读访问当前状态快照（`&dyn State`）
//! - `take_events()` / `poll_events()`：拉取尚未派发的事件（供前端轮询）
//! - `subscribe(callback)`：注册状态变更回调（同步，事件到来时调用）
//! - undo/redo 历史（基于 Task 序列，可选）
//!
//! **纯逻辑，无 I/O、无 GUI 依赖**。所有测试用 MockBackend，`cargo test` 即可跑。
//!
//! 设计要点：
//! - TerminalModel 不直接改 state，state 由 backend 维护；model 只做编排 + 事件聚合
//! - 需要当前激活 pane 的 Task（`needs_active_pane()`），model 从 state 查询后填入
//! - 回调在 `poll_events()` 时同步触发（不在 backend execute 时触发），保证单线程确定性
use crate::core::model::backend::Backend;
use crate::core::model::state::{State, StateChange};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::types::PaneId;
use std::collections::VecDeque;

/// 状态变更回调类型。
pub type StateChangeCallback = Box<dyn Fn(&StateChange) + Send + Sync>;

/// Terminal 层纯逻辑模型。
///
/// 持有一个 backend，编排 task → backend → state → 事件流 → 订阅者。
pub struct TerminalModel {
    backend: Box<dyn Backend>,
    /// 待派发给订阅者的事件队列（从 backend take_events 拉来）。
    pending_events: VecDeque<StateChange>,
    /// 订阅者回调列表。
    subscribers: Vec<StateChangeCallback>,
    /// 已执行的 Task 历史（用于 undo / 调试）。
    history: Vec<Task>,
    /// 是否记录历史（默认 true）。
    record_history: bool,
}

impl TerminalModel {
    /// 创建模型，接管给定 backend。
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Self {
            backend,
            pending_events: VecDeque::new(),
            subscribers: Vec::new(),
            history: Vec::new(),
            record_history: true,
        }
    }

    /// 借用 backend（只读），供测试或前端查询后端状态。
    pub fn backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }

    /// 借用 backend（可变），供测试直接注入事件（模拟 pty 输出等）。
    pub fn backend_mut(&mut self) -> &mut dyn Backend {
        self.backend.as_mut()
    }

    /// 只读访问当前状态（`&dyn State`）。
    pub fn state(&self) -> &dyn State {
        self.backend.as_ref()
    }

    /// 当前后端状态（便捷方法）。
    pub fn backend_status(&self) -> crate::core::model::state::BackendStatus {
        self.backend.backend_status()
    }

    /// 执行一个 Task。
    ///
    /// 若 task 需要「当前激活 pane」（`needs_active_pane`），从 state 查询填入。
    /// 执行后从 backend 拉取事件，放入 pending_events（不立即触发回调，
    /// 等 `take_events` 或 `poll_events` 时统一触发，保证确定性）。
    pub fn execute(&mut self, task: Task) -> anyhow::Result<TaskOutcome> {
        // 补全 needs_active_pane 的 task 的 target
        let resolved = self.resolve_active_pane(task);
        if self.record_history {
            self.history.push(resolved.clone());
        }
        let outcome = self.backend.execute(&resolved)?;
        // 拉取 backend 产生的事件，入队
        let events = self.backend.take_events();
        self.pending_events.extend(events);
        Ok(outcome)
    }

    /// 把需要 active pane 的 task 的 `target: None` 填上当前激活 pane id。
    /// 如果当前没有激活 pane，task 保持原样（backend 会返回 Rejected）。
    ///
    /// 处理规则：
    /// - `SplitPane { target: None }` → 填入 active pane id
    /// - `NextPane` → 用布局树算出下一个 pane，转成 `SwitchPane`
    /// - `PrevPane` → 用布局树算出上一个 pane，转成 `SwitchPane`
    /// - `NewWindow` → 不需要 target（cwd 继承由 backend 内部从 active pane 查询），
    ///   保持原样传给 backend
    fn resolve_active_pane(&self, task: Task) -> Task {
        if !task.needs_active_pane() {
            return task;
        }
        let active = self.state().active_pane().map(|p| p.id);
        match task {
            Task::SplitPane {
                target: None,
                dir,
                command,
                workdir,
            } => match active {
                Some(id) => Task::SplitPane {
                    target: Some(id),
                    dir,
                    command,
                    workdir,
                },
                None => Task::SplitPane {
                    target: None,
                    dir,
                    command,
                    workdir,
                },
            },
            Task::NextPane => match self.next_pane_id() {
                Some(id) => Task::SwitchPane { target: id },
                None => Task::NextPane,
            },
            Task::PrevPane => match self.prev_pane_id() {
                Some(id) => Task::SwitchPane { target: id },
                None => Task::PrevPane,
            },
            // NewWindow 不需要 pane target；cwd 继承由 backend 内部从 active pane 查询。
            // needs_active_pane 返回 true 只是为了提示「需要 active 存在」。
            t => t,
        }
    }

    /// 非阻塞拉取所有 pending 事件，并同步触发订阅者回调。
    /// 返回事件列表（副本），供前端处理。
    pub fn poll_events(&mut self) -> Vec<StateChange> {
        let events: Vec<StateChange> = self.pending_events.drain(..).collect();
        for ev in &events {
            for cb in &self.subscribers {
                cb(ev);
            }
        }
        events
    }

    /// 刷新事件流：先从 backend 拉取最新事件（如 pty 输出）放入 pending，
    /// 再 `poll_events()` 派发给订阅者。
    ///
    /// TUI 等前端在没有新键盘事件时，需要周期性调用此方法以读取 shell 输出；
    /// 否则 `execute()` 之外的 pty 产出（如敲完回车后 shell 的回显/命令输出）
    /// 会一直堆积在 backend 内部缓冲里，永远显示不出来。
    pub fn refresh(&mut self) -> Vec<StateChange> {
        let backend_events = self.backend.take_events();
        self.pending_events.extend(backend_events);
        self.poll_events()
    }

    /// 拉取 pending 事件但不触发回调（供前端自己处理事件分发）。
    pub fn take_events(&mut self) -> Vec<StateChange> {
        self.pending_events.drain(..).collect()
    }

    /// 订阅状态变更。回调在 `poll_events` 时同步调用。
    /// 返回订阅 id（用于 `unsubscribe`）。
    pub fn subscribe(&mut self, cb: StateChangeCallback) -> usize {
        let id = self.subscribers.len();
        self.subscribers.push(cb);
        id
    }

    /// 取消订阅。
    #[allow(unused_must_use)]
    pub fn unsubscribe(&mut self, id: usize) {
        if id < self.subscribers.len() {
            self.subscribers.remove(id);
        }
    }

    /// 已执行的 Task 历史（只读）。
    pub fn history(&self) -> &[Task] {
        &self.history
    }

    /// 是否记录历史。
    pub fn set_record_history(&mut self, on: bool) {
        self.record_history = on;
    }

    /// 连接后端（spawn tmux / 启动本地 shell）。
    pub async fn connect(&mut self) -> anyhow::Result<()> {
        self.backend.connect().await?;
        let events = self.backend.take_events();
        self.pending_events.extend(events);
        Ok(())
    }

    /// 关闭后端。
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.backend.shutdown().await?;
        let events = self.backend.take_events();
        self.pending_events.extend(events);
        Ok(())
    }

    /// 当前激活 pane id（便捷方法）。
    pub fn active_pane_id(&self) -> Option<PaneId> {
        self.state().active_pane().map(|p| p.id)
    }

    /// 当前激活 window 下的所有 pane id（便捷方法）。
    pub fn pane_ids_in_active_tab(&self) -> Vec<PaneId> {
        self.state()
            .active_tab()
            .and_then(|t| self.state().layout(&t.id))
            .map(|tl| tl.tree.leaves())
            .unwrap_or_default()
    }

    /// 下一个 pane id（Alt+] 语义），基于当前激活 window 的布局树。
    pub fn next_pane_id(&self) -> Option<PaneId> {
        let active = self.active_pane_id()?;
        self.state()
            .active_tab()
            .and_then(|t| self.state().layout(&t.id))
            .and_then(|tl| tl.tree.next_leaf(active))
    }

    /// 上一个 pane id（Alt+[ 语义），基于当前激活 window 的布局树。
    pub fn prev_pane_id(&self) -> Option<PaneId> {
        let active = self.active_pane_id()?;
        self.state()
            .active_tab()
            .and_then(|t| self.state().layout(&t.id))
            .and_then(|tl| tl.tree.prev_leaf(active))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::backend::mock::MockBackend;
    use crate::core::model::layout::SplitDir;
    use crate::core::model::state::BackendStatus;
    use crate::core::terminal::input::KeyEvent;
    use crate::core::types::{PaneId, TabId, WindowId};

    use std::sync::{Arc, Mutex};

    fn make_model() -> TerminalModel {
        TerminalModel::new(Box::new(MockBackend::with_single_pane()))
    }

    #[test]
    fn execute_split_resolves_active_pane_target() {
        let mut m = make_model();
        // target: None → model 应从 state 查到 PaneId(1) 填入
        let task = Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        };
        let outcome = m.execute(task).unwrap();
        assert_eq!(outcome, TaskOutcome::Done);
        // history 应记录 resolved 版本（target = Some(1)）
        assert_eq!(m.history().len(), 1);
        match &m.history()[0] {
            Task::SplitPane { target, .. } => {
                assert_eq!(*target, Some(PaneId(1)));
            }
            other => panic!("history 应记录 SplitPane, 得到 {other:?}"),
        }
    }

    #[test]
    fn execute_produces_events_in_pending() {
        let mut m = make_model();
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        // poll_events 应返回 3 个事件并触发回调
        let events = m.poll_events();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], StateChange::PaneAdded { .. }));
        assert!(matches!(events[1], StateChange::LayoutChanged { .. }));
        assert!(matches!(events[2], StateChange::ActivePaneChanged { .. }));
        // 再次 poll 应为空
        assert!(m.poll_events().is_empty());
    }

    #[test]
    fn subscribe_callback_fires_on_poll() {
        let mut m = make_model();
        let fired = Arc::new(Mutex::new(Vec::new()));
        let fired_cb = fired.clone();
        m.subscribe(Box::new(move |ev| {
            fired_cb.lock().unwrap().push(format!("{:?}", ev));
        }));
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        // execute 不触发回调
        assert_eq!(fired.lock().unwrap().len(), 0);
        // poll 触发
        let n = m.poll_events().len();
        assert_eq!(n, 3);
        assert_eq!(fired.lock().unwrap().len(), 3);
    }

    #[test]
    fn take_events_does_not_fire_callbacks() {
        let mut m = make_model();
        let fired = Arc::new(Mutex::new(0u32));
        let fired_cb = fired.clone();
        m.subscribe(Box::new(move |_| {
            *fired_cb.lock().unwrap() += 1;
        }));
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        let events = m.take_events();
        assert_eq!(events.len(), 3);
        assert_eq!(*fired.lock().unwrap(), 0); // 不触发
    }

    /// refresh() 应先从 backend 拉取事件再派发给订阅者。
    /// 验证：execute 后不 poll，backend 仍有 pending；refresh 一次性取走并触发回调。
    #[test]
    fn refresh_pulls_backend_events_and_fires_callbacks() {
        let mut m = make_model();
        let fired = Arc::new(Mutex::new(0u32));
        let fired_cb = fired.clone();
        m.subscribe(Box::new(move |_| {
            *fired_cb.lock().unwrap() += 1;
        }));
        // execute 产生事件但留在 pending（未 poll）
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        // 先 poll 清空 pending（模拟前端已处理 execute 的即时事件）
        let _ = m.poll_events();
        assert_eq!(*fired.lock().unwrap(), 3);

        // 此时 backend 已被 execute 内部 take_events 清空，
        // refresh 应返回空且不重复触发。
        let n = m.refresh().len();
        assert_eq!(n, 0);
        assert_eq!(*fired.lock().unwrap(), 3);
    }

    /// refresh() 在 pending 有事件时应拉取并派发，且排空后为空。
    /// 用 connect 注入事件来验证 refresh 拉取路径（connect 内部 take_events
    /// 到 pending；refresh 再从 backend take + poll）。
    #[tokio::test]
    async fn refresh_drains_pending_and_fires_callbacks() {
        let mut m = make_model();
        let fired = Arc::new(Mutex::new(0u32));
        let fired_cb = fired.clone();
        m.subscribe(Box::new(move |_| {
            *fired_cb.lock().unwrap() += 1;
        }));

        // connect 后 backend 通常会推事件；MockBackend::with_single_pane 已 Connected，
        // connect 是空操作但 shutdown 会推一个 Exited 事件。这里用 shutdown 注入。
        m.shutdown().await.unwrap();
        // shutdown 内部已 take_events 到 pending，但未 poll。
        // refresh 应从 pending 取走并触发回调。
        let events = m.refresh();
        assert!(!events.is_empty());
        let fired_after = *fired.lock().unwrap();
        assert_eq!(fired_after, events.len() as u32);
        // 排空后 refresh 返回空。
        assert!(m.refresh().is_empty());
    }

    #[test]
    fn state_access_after_split() {
        let mut m = make_model();
        assert_eq!(m.state().panes(&TabId(1)).len(), 1);
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = m.poll_events();
        assert_eq!(m.state().panes(&TabId(1)).len(), 2);
        assert_eq!(m.active_pane_id(), Some(PaneId(2)));
    }

    #[test]
    fn history_records_resolved_tasks() {
        let mut m = make_model();
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        })
        .unwrap();
        m.execute(Task::SendKeys {
            target: PaneId(2),
            keys: vec![KeyEvent::Char('l'), KeyEvent::Enter],
        })
        .unwrap();
        assert_eq!(m.history().len(), 2);
    }

    #[test]
    fn set_record_history_off_disables_logging() {
        let mut m = make_model();
        m.set_record_history(false);
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        assert!(m.history().is_empty());
    }

    #[test]
    fn next_prev_pane_id_from_layout() {
        let mut m = make_model();
        // split → [1, 2]
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = m.poll_events();
        // active = 2, next = 1 (循环), prev = 1
        assert_eq!(m.next_pane_id(), Some(PaneId(1)));
        assert_eq!(m.prev_pane_id(), Some(PaneId(1)));
    }

    #[test]
    fn pane_ids_in_active_window() {
        let mut m = make_model();
        assert_eq!(m.pane_ids_in_active_tab(), vec![PaneId(1)]);
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = m.poll_events();
        assert_eq!(m.pane_ids_in_active_tab(), vec![PaneId(1), PaneId(2)]);
    }

    #[tokio::test]
    async fn connect_and_shutdown_flow_events() {
        let mut m = TerminalModel::new(Box::new(MockBackend::new()));
        assert_eq!(m.backend_status(), BackendStatus::Disconnected);
        m.connect().await.unwrap();
        let events = m.poll_events();
        assert!(matches!(
            events[0],
            StateChange::BackendStatusChanged(BackendStatus::Connected)
        ));
        assert_eq!(m.backend_status(), BackendStatus::Connected);
        m.shutdown().await.unwrap();
        let events = m.poll_events();
        assert!(matches!(
            events[0],
            StateChange::BackendStatusChanged(BackendStatus::Exited)
        ));
    }

    #[test]
    fn unsubscribe_removes_callback() {
        let mut m = make_model();
        let fired = Arc::new(Mutex::new(0u32));
        let fired_cb = fired.clone();
        let id = m.subscribe(Box::new(move |_| {
            *fired_cb.lock().unwrap() += 1;
        }));
        m.unsubscribe(id);
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        m.poll_events();
        assert_eq!(*fired.lock().unwrap(), 0);
    }

    #[test]
    fn execute_explicit_target_not_overridden() {
        let mut m = make_model();
        // 显式 target = PaneId(1)，model 不应改它
        m.execute(Task::SplitPane {
            target: Some(PaneId(1)),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        match &m.history()[0] {
            Task::SplitPane { target, .. } => assert_eq!(*target, Some(PaneId(1))),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_state_no_active_pane() {
        let m = TerminalModel::new(Box::new(MockBackend::new()));
        assert!(m.active_pane_id().is_none());
        assert!(m.pane_ids_in_active_tab().is_empty());
        assert!(m.next_pane_id().is_none());
        assert!(m.prev_pane_id().is_none());
    }

    // ── 新增：更多 Task 路径 ─────────────────────────────────────────────

    #[test]
    fn next_pane_resolves_to_switch_pane() {
        let mut m = make_model();
        // split → active=2, next=1 → NextPane 应 resolve 成 SwitchPane{1}
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = m.poll_events();

        m.execute(Task::NextPane).unwrap();
        // history 末项应是 SwitchPane{1}（model 把 NextPane resolve 成 SwitchPane）
        match m.history().last().unwrap() {
            Task::SwitchPane { target } => assert_eq!(*target, PaneId(1)),
            other => panic!("NextPane 应 resolve 成 SwitchPane, 得到 {other:?}"),
        }
        let _ = m.poll_events();
        assert_eq!(m.active_pane_id(), Some(PaneId(1)));
    }

    #[test]
    fn prev_pane_resolves_to_switch_pane() {
        let mut m = make_model();
        // split → active=2, leaves=[1,2], prev(2)=1
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = m.poll_events();

        m.execute(Task::PrevPane).unwrap();
        match m.history().last().unwrap() {
            Task::SwitchPane { target } => assert_eq!(*target, PaneId(1)),
            other => panic!("PrevPane 应 resolve 成 SwitchPane, 得到 {other:?}"),
        }
        let _ = m.poll_events();
        assert_eq!(m.active_pane_id(), Some(PaneId(1)));
    }

    #[test]
    fn close_pane_removes_it_and_updates_active() {
        let mut m = make_model();
        // split → [1, 2], active=2
        m.execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = m.poll_events();

        // close pane 2 → 剩 pane 1，active 回到 1
        m.execute(Task::ClosePane { target: PaneId(2) }).unwrap();
        let events = m.poll_events();
        // 应有 PaneClosed（+ 可能的 LayoutChanged/ActivePaneChanged）
        assert!(events
            .iter()
            .any(|e| matches!(e, StateChange::PaneClosed { pane: PaneId(2) })));
        assert_eq!(m.state().panes(&TabId(1)).len(), 1);
        assert_eq!(m.active_pane_id(), Some(PaneId(1)));
    }

    #[test]
    fn new_window_adds_window_and_first_pane() {
        let mut m = make_model();
        m.execute(Task::NewWindow {
            name: Some("dev".into()),
            command: None,
            workdir: None,
        })
        .unwrap();
        let events = m.poll_events();
        assert!(events
            .iter()
            .any(|e| matches!(e, StateChange::WindowAdded { .. })));
        // 校验：window 数量增加（新 window 有自己的 pane），active 切到新 window
        assert_eq!(m.state().sessions()[0].name, "mock");
        let total_panes = m.state().panes(&TabId(1)).len() + m.state().panes(&TabId(2)).len();
        assert!(total_panes >= 2);
        assert_eq!(m.state().active_window().map(|w| w.id), Some(WindowId(2)));
    }

    #[test]
    fn close_window_removes_all_panes() {
        let mut m = make_model();
        m.execute(Task::NewWindow {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = m.poll_events();
        assert_eq!(m.state().active_window().map(|w| w.id), Some(WindowId(2)));

        m.execute(Task::CloseWindow {
            target: WindowId(2),
        })
        .unwrap();
        let events = m.poll_events();
        assert!(events.iter().any(|e| matches!(
            e,
            StateChange::WindowClosed {
                window: WindowId(2)
            }
        )));
        // active window 应回到 1
        assert_eq!(m.state().active_window().map(|w| w.id), Some(WindowId(1)));
    }

    #[test]
    fn send_keys_appends_to_pane_output() {
        let mut m = make_model();
        m.execute(Task::SendKeys {
            target: PaneId(1),
            keys: vec![KeyEvent::Char('h'), KeyEvent::Char('i'), KeyEvent::Enter],
        })
        .unwrap();
        let _ = m.poll_events();
        // MockBackend 把 SendKeys 累积到 pane 输出
        let out = m.state().pane_output(&PaneId(1)).unwrap();
        assert!(out.ends_with(b"hi\r") || out.ends_with(b"hi"));
    }

    #[test]
    fn write_raw_appends_bytes_to_pane_output() {
        let mut m = make_model();
        m.execute(Task::WriteRaw {
            target: PaneId(1),
            data: b"paste\r\n".to_vec(),
        })
        .unwrap();
        let _ = m.poll_events();
        let out = m.state().pane_output(&PaneId(1)).unwrap();
        assert!(out.windows(6).any(|w| w == b"paste\r") || out.ends_with(b"paste\r\n"));
    }

    #[test]
    fn close_pane_rejects_missing_target() {
        let mut m = make_model();
        let outcome = m.execute(Task::ClosePane { target: PaneId(99) }).unwrap();
        assert!(matches!(outcome, TaskOutcome::Rejected { .. }));
        let events = m.poll_events();
        assert!(events.is_empty());
    }

    #[test]
    fn switch_pane_to_missing_target_rejected() {
        let mut m = make_model();
        let outcome = m.execute(Task::SwitchPane { target: PaneId(99) }).unwrap();
        assert!(matches!(outcome, TaskOutcome::Rejected { .. }));
    }

    #[tokio::test]
    async fn shutdown_task_pushes_exited_event() {
        let mut m = make_model();
        m.execute(Task::Shutdown).unwrap();
        let events = m.poll_events();
        assert!(events
            .iter()
            .any(|e| matches!(e, StateChange::BackendStatusChanged(BackendStatus::Exited))));
        assert_eq!(m.backend_status(), BackendStatus::Exited);
    }

    #[test]
    fn rename_window_event_emitted() {
        let mut m = make_model();
        m.execute(Task::RenameWindow {
            target: WindowId(1),
            name: "renamed".into(),
        })
        .unwrap();
        let events = m.poll_events();
        assert!(events.iter().any(|e| matches!(
            e,
            StateChange::WindowRenamed {
                window: WindowId(1),
                ..
            }
        )));
        assert_eq!(m.state().active_window().unwrap().name, "renamed");
    }

    #[test]
    fn resize_pane_updates_cols_rows() {
        let mut m = make_model();
        m.execute(Task::ResizePane {
            target: PaneId(1),
            cols: 120,
            rows: 40,
        })
        .unwrap();
        let _ = m.poll_events();
        let p = m.state().pane(&PaneId(1)).unwrap();
        assert_eq!(p.cols, 120);
        assert_eq!(p.rows, 40);
    }
}
