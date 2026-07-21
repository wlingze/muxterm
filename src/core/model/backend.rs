//! Backend trait：一个统一的终端后端抽象。
//!
//! TmuxBackend 和 LocalBackend 都实现此 trait，TerminalModel 持有 `Box<dyn Backend>`。
//!
//! 设计要点：
//! - Backend 维护并 `&mut self` 更新内部 State，实现 `State` trait 的只读视图。
//! - Backend 接收 `Task`，把它映射到具体动作（tmux 命令 / 本地 spawn）。
//! - Backend 通过通道推送 `StateChange` 事件（异步），前端/TerminalModel 订阅。
//! - 连接（connect）和关闭（shutdown）是异步方法。
//! - 协议解析器（`tmux::protocol`）、命令构造器（`tmux::command`）是 Backend 的内部实现细节，不暴露给 TerminalModel。
use crate::core::model::state::{BackendStatus, State, StateChange};
use crate::core::model::task::{Task, TaskOutcome};
use async_trait::async_trait;

/// 终端后端 trait。
///
/// 一个 Backend 实例 = 一个 session 来源（本地 tmux / 远程 ssh tmux / 纯本地 shell）。
/// 同一时刻可能存在多个 Backend（多 session），TerminalModel 聚合它们的 State。
///
/// 生命周期：
/// 1. `connect()` — 建立 connection / spawn tmux -CC / 初始化本地 shell
/// 2. `execute(Task)` — 反复执行任务
/// 3. `poll_events()` / `take_events()` — 取状态变更事件
/// 4. `shutdown()` — detach / kill / 关闭所有子进程
#[async_trait]
pub trait Backend: State {
    /// 建立连接（spawn tmux / 启动本地 shell）。
    /// 成功后 `status()` 应为 `Connected`。
    async fn connect(&mut self) -> anyhow::Result<()>;

    /// 同步执行一个 Task（不阻塞事件循环；内部若需 I/O 用 `tokio::spawn` 后台执行）。
    /// 返回 `Ok(Done)` 表示已派发；`Ok(Rejected{..})` 表示目标/状态不允许。
    /// 状态变更通过随后的事件流（`take_events`）推送。
    fn execute(&mut self, task: &Task) -> anyhow::Result<TaskOutcome>;

    /// 非阻塞拉取所有尚未消费的状态变更事件（FIFO）。
    /// 前端用 `glib::timeout_add_local` 16ms 轮询；TerminalModel 也用它聚合。
    fn take_events(&mut self) -> Vec<StateChange>;

    /// 当前后端状态（`State::status` 的便捷别名，语义一致）。
    fn backend_status(&self) -> BackendStatus {
        self.status()
    }

    /// 关闭后端：detach（tmux）/ kill 所有子进程（local）。
    /// 关闭后 `status()` 应为 `Exited` 或 `Disconnected`。
    async fn shutdown(&mut self) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::layout::SplitDir;
    use crate::core::model::layout::{LayoutNode, WindowLayout};
    use crate::core::model::state::{PaneInfo, SessionInfo, WindowInfo};
    use crate::core::model::task::Task;
    use crate::core::types::{PaneId, SessionId, WindowId};

    /// 最小可用的 mock backend，用于 trait 编译检查 + TerminalModel 单元测试。
    pub struct MockBackend {
        sessions: Vec<SessionInfo>,
        windows: Vec<WindowInfo>,
        panes: Vec<PaneInfo>,
        layouts: Vec<WindowLayout>,
        outputs: Vec<(PaneId, Vec<u8>)>,
        status: BackendStatus,
        events: Vec<StateChange>,
        pub executed: Vec<Task>,
    }

    impl MockBackend {
        pub fn new() -> Self {
            Self {
                sessions: vec![],
                windows: vec![],
                panes: vec![],
                layouts: vec![],
                outputs: vec![],
                status: BackendStatus::Disconnected,
                events: vec![],
                executed: vec![],
            }
        }

        /// 预置一个单 pane 的最简状态，并标记 Connected。
        pub fn with_single_pane() -> Self {
            let mut b = Self::new();
            b.sessions.push(SessionInfo {
                id: SessionId(1),
                name: "mock".into(),
                active_window: Some(WindowId(1)),
            });
            b.windows.push(WindowInfo {
                id: WindowId(1),
                name: "w1".into(),
                session: SessionId(1),
                active: true,
            });
            b.panes.push(PaneInfo {
                id: PaneId(1),
                window: WindowId(1),
                active: true,
                title: "bash".into(),
                cols: 80,
                rows: 24,
            });
            b.layouts.push(WindowLayout {
                window: WindowId(1),
                tree: LayoutNode::leaf(PaneId(1)),
                active: PaneId(1),
            });
            b.outputs.push((PaneId(1), Vec::new()));
            b.status = BackendStatus::Connected;
            b
        }
    }

    impl State for MockBackend {
        fn sessions(&self) -> &[SessionInfo] {
            &self.sessions
        }
        fn active_session(&self) -> Option<&SessionInfo> {
            self.sessions.first()
        }
        fn active_window(&self) -> Option<&WindowInfo> {
            self.windows.iter().find(|w| w.active)
        }
        fn active_pane(&self) -> Option<&PaneInfo> {
            self.panes.iter().find(|p| p.active)
        }
        fn layout(&self, window: &WindowId) -> Option<&WindowLayout> {
            self.layouts.iter().find(|l| &l.window == window)
        }
        fn panes(&self, window: &WindowId) -> Vec<&PaneInfo> {
            self.panes.iter().filter(|p| &p.window == window).collect()
        }
        fn pane(&self, pane: &PaneId) -> Option<&PaneInfo> {
            self.panes.iter().find(|p| &p.id == pane)
        }
        fn pane_output(&self, pane: &PaneId) -> Option<&[u8]> {
            self.outputs
                .iter()
                .find(|(pid, _)| pid == pane)
                .map(|(_, v)| v.as_slice())
        }
        fn status(&self) -> BackendStatus {
            self.status
        }
    }

    #[async_trait]
    impl Backend for MockBackend {
        async fn connect(&mut self) -> anyhow::Result<()> {
            self.status = BackendStatus::Connected;
            self.events
                .push(StateChange::BackendStatusChanged(BackendStatus::Connected));
            Ok(())
        }

        fn execute(&mut self, task: &Task) -> anyhow::Result<TaskOutcome> {
            self.executed.push(task.clone());
            // mock：SplitPane 简单追加一个 pane + 事件
            if let Task::SplitPane { target, dir, .. } = task {
                let target = target.unwrap_or(PaneId(1));
                // 找到 target 所在 window
                let win_id = self
                    .panes
                    .iter()
                    .find(|p| p.id == target)
                    .map(|p| p.window)
                    .unwrap_or(WindowId(1));
                let new_id = PaneId(self.panes.iter().map(|p| p.id.0).max().unwrap_or(0) + 1);
                // 取消旧 active，设置新 pane 为 active
                for p in self.panes.iter_mut() {
                    if p.window == win_id {
                        p.active = p.id == new_id;
                    }
                }
                self.panes.push(PaneInfo {
                    id: new_id,
                    window: win_id,
                    active: true,
                    title: "bash".into(),
                    cols: 40,
                    rows: 24,
                });
                // 更新布局树
                if let Some(wl) = self.layouts.iter_mut().find(|l| l.window == win_id) {
                    wl.tree.split_at(target, new_id, *dir);
                    wl.active = new_id;
                    self.events.push(StateChange::PaneAdded {
                        pane: new_id,
                        window: win_id,
                    });
                    self.events.push(StateChange::LayoutChanged {
                        window: win_id,
                        layout: wl.clone(),
                    });
                    self.events.push(StateChange::ActivePaneChanged {
                        window: win_id,
                        pane: new_id,
                    });
                }
            }
            Ok(TaskOutcome::Done)
        }

        fn take_events(&mut self) -> Vec<StateChange> {
            std::mem::take(&mut self.events)
        }

        async fn shutdown(&mut self) -> anyhow::Result<()> {
            self.status = BackendStatus::Exited;
            self.events
                .push(StateChange::BackendStatusChanged(BackendStatus::Exited));
            Ok(())
        }
    }

    #[tokio::test]
    async fn mock_backend_connect_and_split() {
        let mut b = MockBackend::with_single_pane();
        assert_eq!(b.backend_status(), BackendStatus::Connected);
        let events = b.take_events();
        assert!(events.is_empty()); // with_single_pane 不预置事件

        // 执行 split
        let task = Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        };
        let outcome = b.execute(&task).unwrap();
        assert_eq!(outcome, TaskOutcome::Done);
        assert_eq!(b.executed.len(), 1);

        // 应该产生 3 个事件：PaneAdded、LayoutChanged、ActivePaneChanged
        let events = b.take_events();
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0],
            StateChange::PaneAdded {
                pane: PaneId(2),
                ..
            }
        ));
        assert!(matches!(events[1], StateChange::LayoutChanged { .. }));
        assert!(matches!(
            events[2],
            StateChange::ActivePaneChanged {
                pane: PaneId(2),
                ..
            }
        ));

        // 验证状态更新
        assert_eq!(b.panes(&WindowId(1)).len(), 2);
        assert_eq!(b.active_pane().unwrap().id, PaneId(2));
    }

    #[tokio::test]
    async fn mock_backend_shutdown() {
        let mut b = MockBackend::with_single_pane();
        b.shutdown().await.unwrap();
        assert_eq!(b.backend_status(), BackendStatus::Exited);
        let events = b.take_events();
        assert!(matches!(
            events[0],
            StateChange::BackendStatusChanged(BackendStatus::Exited)
        ));
    }

    #[test]
    fn mock_backend_take_events_drains() {
        let mut b = MockBackend::with_single_pane();
        b.events.push(StateChange::SessionsChanged);
        b.events.push(StateChange::SessionsChanged);
        assert_eq!(b.take_events().len(), 2);
        assert!(b.take_events().is_empty()); // 已排空
    }
}
