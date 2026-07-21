//! 终端状态快照类型 + `State` trait。
//!
//! `State` 描述「当前 session/window/pane 的完整快照」，由 Backend 维护，
//! 被 TerminalModel 和前端读取。纯数据接口，**不依赖任何 I/O 或 GUI**。
//!
//! 设计要点：
//! - 前端只读 `State` 做渲染；状态变更通过 `BackendEvent` 推送（见 `backend.rs`）。
//! - pane 输出是字节流（`&[u8]`），因为可能含非 UTF-8 的 ANSI 序列。
//! - 所有方法返回 `Option` / `&` 引用，不 clone，便于高频渲染。
use crate::core::model::layout::WindowLayout;
use crate::core::types::{PaneId, SessionId, WindowId};

/// 一个 session 的元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: SessionId,
    pub name: String,
    /// 该 session 当前激活的 window（若有）。
    pub active_window: Option<WindowId>,
}

/// 一个 window 的元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub name: String,
    /// 所属 session。
    pub session: SessionId,
    /// 是否激活（在所属 session 中）。
    pub active: bool,
}

/// 一个 pane 的元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: PaneId,
    /// 所属 window。
    pub window: WindowId,
    /// 是否激活（在所属 window 中）。
    pub active: bool,
    /// pane 标题（通常 = 当前进程名），由 Backend 更新。
    pub title: String,
    /// pane 的字符格尺寸（由 Backend 从 tmux layout 或 vte4 同步）。
    pub cols: u16,
    pub rows: u16,
}

/// 状态变更事件（Backend → TerminalModel → 前端）。
///
/// 细粒度事件，避免每次小变动都全量重渲染。前端可按需聚合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateChange {
    /// 某 pane 有新输出（增量字节）。
    PaneOutput {
        pane: PaneId,
        /// 自上次事件以来的增量字节。
        data: Vec<u8>,
    },
    /// 新 window 被加入。
    WindowAdded {
        window: WindowId,
        session: SessionId,
    },
    /// window 被关闭。
    WindowClosed { window: WindowId },
    /// window 被重命名。
    WindowRenamed { window: WindowId, name: String },
    /// window 布局变化（分割 / resize / pane 增减）。
    LayoutChanged {
        window: WindowId,
        /// 新布局树（完整快照，非增量）。
        layout: WindowLayout,
    },
    /// pane 被加入（split 的结果，或 tmux 新建 pane）。
    PaneAdded { pane: PaneId, window: WindowId },
    /// pane 被关闭。
    PaneClosed { pane: PaneId },
    /// pane 标题变化。
    PaneTitleChanged { pane: PaneId, title: String },
    /// pane 尺寸变化。
    PaneResized { pane: PaneId, cols: u16, rows: u16 },
    /// 激活的 pane 变化。
    ActivePaneChanged { window: WindowId, pane: PaneId },
    /// 激活的 window 变化。
    ActiveWindowChanged {
        session: SessionId,
        window: WindowId,
    },
    /// 当前 session 变化。
    SessionChanged {
        session: SessionId,
        name: Option<String>,
    },
    /// session 被重命名。
    SessionRenamed { session: SessionId, name: String },
    /// session 列表变化。
    SessionsChanged,
    /// 后端整体状态变化（连接中 / 已连接 / 断开 / 错误）。
    BackendStatusChanged(BackendStatus),
}

/// 后端连接/运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendStatus {
    /// 未连接（初始 / 已断开）。
    Disconnected,
    /// 正在连接 / spawn 中。
    Connecting,
    /// 已连接，正常工作。
    Connected,
    /// 出错（附带消息，通过单独事件传 detail，这里只标状态）。
    Error,
    /// 后端进程已退出。
    Exited,
}

/// 状态快照 trait：只读访问当前终端状态。
///
/// Backend 实现此 trait，TerminalModel 持有 `&dyn State`，前端也只读 `State`。
/// 所有方法不可失败（找不到返回 `None`），调用方自行处理。
pub trait State {
    /// 当前所有 session。
    fn sessions(&self) -> &[SessionInfo];

    /// 当前激活的 session。
    fn active_session(&self) -> Option<&SessionInfo>;

    /// 当前激活的 window（属于激活 session）。
    fn active_window(&self) -> Option<&WindowInfo>;

    /// 当前激活的 pane（属于激活 window）。
    fn active_pane(&self) -> Option<&PaneInfo>;

    /// 某 window 的布局树。
    fn layout(&self, window: &WindowId) -> Option<&WindowLayout>;

    /// 某 window 下的所有 pane。
    fn panes(&self, window: &WindowId) -> Vec<&PaneInfo>;

    /// 某 pane 的元信息。
    fn pane(&self, pane: &PaneId) -> Option<&PaneInfo>;

    /// pane 的累计输出字节流（scrollback + 当前可见）。
    /// 返回的是累积快照；前端可自行维护增量。
    fn pane_output(&self, pane: &PaneId) -> Option<&[u8]>;

    /// 后端连接状态。
    fn status(&self) -> BackendStatus;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::layout::LayoutNode;

    /// 一个最小可用的内存 State 实现，用于 trait 编译期检查 + 后续 mock。
    struct MemState {
        sessions: Vec<SessionInfo>,
        windows: Vec<WindowInfo>,
        panes: Vec<PaneInfo>,
        layouts: Vec<WindowLayout>,
        outputs: Vec<(PaneId, Vec<u8>)>,
        status: BackendStatus,
        active_session: Option<SessionId>,
        active_window: Option<WindowId>,
        active_pane: Option<PaneId>,
    }

    impl State for MemState {
        fn sessions(&self) -> &[SessionInfo] {
            &self.sessions
        }
        fn active_session(&self) -> Option<&SessionInfo> {
            self.active_session
                .and_then(|sid| self.sessions.iter().find(|s| s.id == sid))
        }
        fn active_window(&self) -> Option<&WindowInfo> {
            self.active_window
                .and_then(|wid| self.windows.iter().find(|w| w.id == wid))
        }
        fn active_pane(&self) -> Option<&PaneInfo> {
            self.active_pane
                .and_then(|pid| self.panes.iter().find(|p| p.id == pid))
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

    #[test]
    fn mem_state_basic_access() {
        let s = MemState {
            sessions: vec![SessionInfo {
                id: SessionId(1),
                name: "main".into(),
                active_window: Some(WindowId(1)),
            }],
            windows: vec![WindowInfo {
                id: WindowId(1),
                name: "bash".into(),
                session: SessionId(1),
                active: true,
            }],
            panes: vec![PaneInfo {
                id: PaneId(1),
                window: WindowId(1),
                active: true,
                title: "bash".into(),
                cols: 80,
                rows: 24,
            }],
            layouts: vec![WindowLayout {
                window: WindowId(1),
                tree: LayoutNode::leaf(PaneId(1)),
                active: PaneId(1),
            }],
            outputs: vec![(PaneId(1), b"hello".to_vec())],
            status: BackendStatus::Connected,
            active_session: Some(SessionId(1)),
            active_window: Some(WindowId(1)),
            active_pane: Some(PaneId(1)),
        };
        assert_eq!(s.sessions().len(), 1);
        assert_eq!(s.active_session().unwrap().name, "main");
        assert_eq!(s.active_window().unwrap().name, "bash");
        assert_eq!(s.active_pane().unwrap().title, "bash");
        assert_eq!(s.panes(&WindowId(1)).len(), 1);
        assert!(s.layout(&WindowId(1)).is_some());
        assert_eq!(s.pane(&PaneId(1)).unwrap().cols, 80);
        assert_eq!(s.pane_output(&PaneId(1)).unwrap(), b"hello");
        assert_eq!(s.status(), BackendStatus::Connected);
    }

    #[test]
    fn state_change_variants_compile() {
        // 仅验证枚举可构造、可比较，不依赖运行时行为。
        let c1 = StateChange::WindowAdded {
            window: WindowId(1),
            session: SessionId(1),
        };
        let c2 = StateChange::PaneOutput {
            pane: PaneId(1),
            data: vec![0x1b, b'['],
        };
        assert_eq!(c1, dup(&c1));
        assert_eq!(c2, dup(&c2));
    }

    fn dup(c: &StateChange) -> StateChange {
        c.clone()
    }

    #[test]
    fn empty_state_returns_none() {
        let s = MemState {
            sessions: vec![],
            windows: vec![],
            panes: vec![],
            layouts: vec![],
            outputs: vec![],
            status: BackendStatus::Disconnected,
            active_session: None,
            active_window: None,
            active_pane: None,
        };
        assert!(s.sessions().is_empty());
        assert!(s.active_session().is_none());
        assert!(s.active_window().is_none());
        assert!(s.active_pane().is_none());
        assert!(s.layout(&WindowId(1)).is_none());
        assert!(s.pane(&PaneId(1)).is_none());
        assert!(s.pane_output(&PaneId(1)).is_none());
        assert_eq!(s.status(), BackendStatus::Disconnected);
    }
}
