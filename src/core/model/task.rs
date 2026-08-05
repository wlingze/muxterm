//! Task：可执行操作的纯描述。
//!
//! `Task` 是「用户/前端想做的事」的抽象描述，**不含任何执行逻辑**。
//! TerminalModel 把 `Task` 交给 Backend，Backend 把它映射到具体动作
//! （tmux 命令 / 本地 spawn / vte4 操作）。
//!
//! Task 是 `Copy`/`Clone` 友好的纯数据，可序列化、可记入历史（undo/redo）。
//! 所有 Task 都针对「当前激活的 pane/window/session」，除非显式指定 target。
use crate::core::model::layout::SplitDir;
use crate::core::protocol::terminal::input::KeyEvent;
use crate::core::types::{PaneId, TabId, WindowId};

/// 所有终端操作任务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    // ── 布局操作 ───────────────────────────────────────────
    /// 分割当前激活 pane（或指定 target）。
    SplitPane {
        target: Option<PaneId>,
        dir: SplitDir,
        /// 新 pane 启动程序；None = 用默认（`pane.default_command`）。
        command: Option<Vec<String>>,
        /// 新 pane 工作目录；None = 继承当前 pane 的 cwd。
        workdir: Option<String>,
    },
    /// 关闭指定 pane（发 SIGHUP / kill-pane）。
    ClosePane { target: PaneId },
    /// resize pane 到指定字符格尺寸。
    ResizePane {
        target: PaneId,
        cols: u16,
        rows: u16,
    },
    /// resize tmux 控制 client；与 pane resize 分开，避免 GUI 逐 pane 触发布局反馈。
    ResizeClient { cols: u16, rows: u16 },
    /// resize 分割条相邻 pane 的单一轴，用于 GUI 鼠标拖拽。
    ResizePaneAxis {
        target: PaneId,
        dir: SplitDir,
        size: u16,
    },
    /// resize pane 的某一方向（增量，正=变大，负=变小），用于拖拽分割条。
    ResizePaneStep {
        target: PaneId,
        dir: SplitDir,
        delta: i32,
    },

    // ── 焦点切换 ───────────────────────────────────────────
    /// 切换激活 pane（同 window 内）。
    SwitchPane { target: PaneId },
    /// 切换到下一个 pane（循环）。
    NextPane,
    /// 切换到上一个 pane（循环）。
    PrevPane,

    // ── Tab / Window 操作 ─────────────────────────────────
    /// 新建 window（tab），可选名字。新建后焦点跳到新 window。
    NewWindow {
        name: Option<String>,
        /// 新 window 第一个 pane 的启动程序；None = 默认。
        command: Option<Vec<String>>,
        workdir: Option<String>,
    },
    /// 关闭 window（含所有 tab/pane）。
    CloseWindow { target: WindowId },
    /// 切换激活 window（同 session 内）。
    SwitchWindow { target: WindowId },
    /// 重命名 window。
    RenameWindow { target: WindowId, name: String },

    // ── Tab 操作 ───────────────────────────────────────────
    /// 在 window 内新建 tab。新建后焦点跳到新 tab。
    NewTab {
        window: WindowId,
        name: Option<String>,
        /// 新 tab 第一个 pane 的启动程序；None = 默认。
        command: Option<Vec<String>>,
        workdir: Option<String>,
    },
    /// 关闭 tab（含所有 pane）。
    CloseTab { target: TabId },
    /// 切换激活 tab（同 window 内）。
    SwitchTab { target: TabId },
    /// 重命名 tab。
    RenameTab { target: TabId, name: String },

    // ── Session 操作 ──────────────────────────────────────
    /// 切换激活 session。
    SwitchSession {
        target: crate::core::types::SessionId,
    },
    /// 重命名 session。
    RenameSession {
        target: crate::core::types::SessionId,
        name: String,
    },

    // ── 输入 ──────────────────────────────────────────────
    /// 向 pane 发送按键序列（tmux send-keys / 本地 pty write）。
    SendKeys { target: PaneId, keys: Vec<KeyEvent> },
    /// 向 pane 写入原始字节（不经过按键编码，用于粘贴）。
    WriteRaw { target: PaneId, data: Vec<u8> },

    // ── 生命周期 ──────────────────────────────────────────
    /// 分离当前控制 client，但保留 tmux session / daemon 继续运行。
    Detach,
    /// 关闭整个后端并释放资源（tmux 后端会清理 control client）。
    Shutdown,
}

impl Task {
    /// 该任务是否需要「当前激活 pane」作为默认 target。
    pub fn needs_active_pane(&self) -> bool {
        matches!(
            self,
            Task::SplitPane { target: None, .. }
                | Task::NextPane
                | Task::PrevPane
                | Task::NewWindow { .. }
        )
    }

    /// 该任务是否是只读的（不改布局/进程，只切换焦点）。
    pub fn is_readonly(&self) -> bool {
        matches!(
            self,
            Task::SwitchPane { .. }
                | Task::NextPane
                | Task::PrevPane
                | Task::SwitchWindow { .. }
                | Task::SwitchSession { .. }
        )
    }
}

/// Task 执行结果。
///
/// 大多数 Task 成功时无返回值（`Ok(())`）；查询类任务（未来扩展）可返回数据。
/// 目前所有 Task 都返回 `()`，保留枚举以便未来需要时扩展。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    /// 成功，状态已更新（通过 StateChange 事件推送细节）。
    Done,
    /// 被拒绝（target 不存在 / 后端未连接 / 配置不允许）。
    Rejected { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::protocol::terminal::input::ArrowDir;

    fn pid(n: u32) -> PaneId {
        PaneId(n)
    }

    #[test]
    fn split_pane_default_target_needs_active() {
        let t = Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        };
        assert!(t.needs_active_pane());
        assert!(!t.is_readonly());
    }

    #[test]
    fn split_pane_explicit_target_no_active_needed() {
        let t = Task::SplitPane {
            target: Some(pid(1)),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        };
        assert!(!t.needs_active_pane());
    }

    #[test]
    fn switch_pane_is_readonly() {
        let t = Task::SwitchPane { target: pid(2) };
        assert!(t.is_readonly());
        assert!(!t.needs_active_pane());
    }

    #[test]
    fn next_prev_pane_need_active_and_readonly() {
        assert!(Task::NextPane.needs_active_pane());
        assert!(Task::NextPane.is_readonly());
        assert!(Task::PrevPane.needs_active_pane());
        assert!(Task::PrevPane.is_readonly());
    }

    #[test]
    fn new_window_needs_active_for_inherited_cwd() {
        let t = Task::NewWindow {
            name: None,
            command: None,
            workdir: None,
        };
        assert!(t.needs_active_pane());
    }

    #[test]
    fn send_keys_not_readonly() {
        let t = Task::SendKeys {
            target: pid(1),
            keys: vec![KeyEvent::Char('a'), KeyEvent::Enter],
        };
        assert!(!t.is_readonly());
        assert!(!t.needs_active_pane());
    }

    #[test]
    fn write_raw_carries_bytes() {
        let t = Task::WriteRaw {
            target: pid(1),
            data: b"paste\r\n".to_vec(),
        };
        assert!(!t.is_readonly());
    }

    #[test]
    fn task_is_clone_and_eq() {
        let t1 = Task::ClosePane { target: pid(1) };
        let t2 = t1.clone();
        assert_eq!(t1, t2);
    }

    #[test]
    fn resize_step_signed_delta() {
        let t = Task::ResizePaneStep {
            target: pid(1),
            dir: SplitDir::Horizontal,
            delta: -5,
        };
        assert!(!t.is_readonly());
    }

    #[test]
    fn shutdown_is_not_readonly() {
        assert!(!Task::Shutdown.is_readonly());
    }

    #[test]
    fn detach_is_not_shutdown() {
        assert_ne!(Task::Detach, Task::Shutdown);
        assert!(!Task::Detach.is_readonly());
    }

    #[test]
    fn task_outcome_variants() {
        let d = TaskOutcome::Done;
        let r = TaskOutcome::Rejected {
            reason: "no such pane".into(),
        };
        assert_ne!(d, r);
    }

    #[test]
    fn key_event_arrow_roundtrips_in_sendkeys() {
        let keys = vec![
            KeyEvent::Arrow(ArrowDir::Up),
            KeyEvent::Char('x'),
            KeyEvent::Ctrl('c'),
        ];
        let t = Task::SendKeys {
            target: pid(3),
            keys,
        };
        if let Task::SendKeys { keys, .. } = t {
            assert_eq!(keys.len(), 3);
        } else {
            panic!("不是 SendKeys");
        }
    }
}
