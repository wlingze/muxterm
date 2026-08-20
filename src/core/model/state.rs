//! 终端状态快照类型 + `State` trait（产品模型：Workspace → Tab → Pane）。
//!
//! `State` 描述「当前 Workspace 的完整快照」，由 Runtime 维护，
//! 被 TerminalModel 和前端读取。纯数据接口，**不依赖任何 I/O 或 GUI**。
//!
//! 设计要点：
//! - 前端只读 `State` 做渲染；状态变更通过 `StateChange` 推送。
//! - pane 输出是字节流（`&[u8]`），因为可能含非 UTF-8 的 ANSI 序列。
//! - 所有方法返回 `Option` / `&` 引用，不 clone，便于高频渲染。
use crate::core::types::{PaneId, TabId};

/// 工作区元信息（Runtime 侧可知的部分：名字与 runtime 种类）。
/// `WorkspaceId` / transport 属于池层，由 WorkspacePool 持有。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceInfo {
    pub name: String,
    pub runtime: String,
    /// 该工作区当前激活的 tab（若有）。
    pub active_tab: Option<TabId>,
}

/// 一个 tab 的元信息。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TabInfo {
    pub id: TabId,
    pub name: String,
    /// 是否激活（在所属 Workspace 中）。
    pub active: bool,
}

/// 一个 pane 的元信息。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneInfo {
    pub id: PaneId,
    /// 所属 tab。
    pub tab: TabId,
    /// 是否激活（在所属 window 中）。
    pub active: bool,
    /// pane 标题（通常 = 当前进程名），由 Runtime 更新。
    pub title: String,
    /// pane 的字符格尺寸（由 Runtime 从 tmux layout 或 vte4 同步）。
    pub cols: u16,
    pub rows: u16,
}

/// 状态变更事件（Runtime → TerminalModel → 前端）。
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
    /// 某 pane 的权威屏幕快照（重置 VT 后一次性应用，不是增量）。
    ///
    /// tmux control mode 在 pause、重连或 attach seed 后可能丢弃控制 client
    /// 尚未发送的 blocks。此事件让索引面和 GUI 以同一份 snapshot 重新对齐，
    /// 后续 `PaneOutput` 才继续按增量消费。
    PaneSnapshot { pane: PaneId, data: Vec<u8> },
    /// tab 被加入。
    TabAdded { tab: TabId },
    /// tab 被关闭。
    TabClosed { tab: TabId },
    /// tab 被重命名。
    TabRenamed { tab: TabId, name: String },
    /// 激活的 tab 变化。
    ActiveTabChanged { tab: TabId },
    /// tab 布局变化（分割 / resize / pane 增减）。
    LayoutChanged {
        tab: TabId,
        /// 新布局树（完整快照，非增量）。
        layout: crate::core::model::layout::TabLayout,
    },
    /// pane 被加入（split 的结果，或 tmux 新建 pane）。
    PaneAdded { pane: PaneId, tab: TabId },
    /// pane 被关闭。
    PaneClosed { pane: PaneId },
    /// pane 标题变化。
    PaneTitleChanged { pane: PaneId, title: String },
    /// pane 尺寸变化。
    PaneResized { pane: PaneId, cols: u16, rows: u16 },
    /// 激活的 pane 变化。
    ActivePaneChanged { tab: TabId, pane: PaneId },
    /// 工作区被重命名。
    WorkspaceRenamed { name: String },
    /// 池里工作区列表变化（open / close / evict）。
    PoolChanged,
    /// status bar 订阅推送（`refresh-client -B` → `%subscription-changed`）。
    /// name 是订阅名（如 `muxterm.status-left`），value 是 format 展开值；
    /// pane 是订阅元数据里的 pane-id（status-left/right 为 None）。
    StatusBarSubscription {
        name: String,
        value: String,
        pane: Option<PaneId>,
    },
    /// 后端整体状态变化（连接中 / 已连接 / 断开 / 错误）。
    BackendStatusChanged(BackendStatus),
}

/// 后端连接/运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
/// Runtime 实现此 trait，TerminalModel 持有 `&dyn State`，前端也只读 `State`。
/// 所有方法不可失败（找不到返回 `None`），调用方自行处理。
pub trait State {
    /// 工作区名字（tmux 时常用 session 名；shell 用目录名/配置名）。
    fn workspace_name(&self) -> &str;

    /// 工作区 runtime 种类（`"tmux"` / `"shell"`）。
    fn workspace_runtime(&self) -> &str;

    /// 当前激活的 tab（属于激活 window）。
    fn active_tab(&self) -> Option<&TabInfo>;

    /// 当前激活的 pane（属于激活 tab）。
    fn active_pane(&self) -> Option<&PaneInfo>;

    /// 工作区里所有 tab。
    fn tabs(&self) -> Vec<&TabInfo>;

    /// 某 tab 的元信息。
    fn tab(&self, tab: &TabId) -> Option<&TabInfo>;

    /// 某 tab 的布局树。
    fn layout(&self, tab: &TabId) -> Option<&crate::core::model::layout::TabLayout>;

    /// 某 tab 下的所有 pane。
    fn panes(&self, tab: &TabId) -> Vec<&PaneInfo>;

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
        workspace_name: String,
        workspace_runtime: String,
        tabs: Vec<TabInfo>,
        panes: Vec<PaneInfo>,
        layouts: Vec<crate::core::model::layout::TabLayout>,
        outputs: Vec<(PaneId, Vec<u8>)>,
        status: BackendStatus,
        active_tab: Option<TabId>,
        active_pane: Option<PaneId>,
    }

    impl State for MemState {
        fn workspace_name(&self) -> &str {
            &self.workspace_name
        }
        fn workspace_runtime(&self) -> &str {
            &self.workspace_runtime
        }
        fn active_tab(&self) -> Option<&TabInfo> {
            self.active_tab
                .and_then(|tid| self.tabs.iter().find(|t| t.id == tid))
        }
        fn active_pane(&self) -> Option<&PaneInfo> {
            self.active_pane
                .and_then(|pid| self.panes.iter().find(|p| p.id == pid))
        }
        fn tabs(&self) -> Vec<&TabInfo> {
            self.tabs.iter().collect()
        }
        fn tab(&self, tab: &TabId) -> Option<&TabInfo> {
            self.tabs.iter().find(|t| &t.id == tab)
        }
        fn layout(&self, tab: &TabId) -> Option<&crate::core::model::layout::TabLayout> {
            self.layouts.iter().find(|l| &l.tab == tab)
        }
        fn panes(&self, tab: &TabId) -> Vec<&PaneInfo> {
            self.panes.iter().filter(|p| &p.tab == tab).collect()
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
            workspace_name: "main".into(),
            workspace_runtime: "tmux".into(),
            tabs: vec![TabInfo {
                id: TabId(1),
                name: "shell".into(),
                active: true,
            }],
            panes: vec![PaneInfo {
                id: PaneId(1),
                tab: TabId(1),
                active: true,
                title: "bash".into(),
                cols: 80,
                rows: 24,
            }],
            layouts: vec![crate::core::model::layout::TabLayout {
                tab: TabId(1),
                tree: LayoutNode::leaf(PaneId(1)),
                active: PaneId(1),
            }],
            outputs: vec![(PaneId(1), b"hello".to_vec())],
            status: BackendStatus::Connected,
            active_tab: Some(TabId(1)),
            active_pane: Some(PaneId(1)),
        };
        assert_eq!(s.workspace_name(), "main");
        assert_eq!(s.workspace_runtime(), "tmux");
        assert_eq!(s.active_pane().unwrap().title, "bash");
        assert_eq!(s.panes(&TabId(1)).len(), 1);
        assert!(s.layout(&TabId(1)).is_some());
        assert_eq!(s.active_tab().unwrap().name, "shell");
        assert_eq!(s.tabs().len(), 1);
        assert_eq!(s.pane(&PaneId(1)).unwrap().cols, 80);
        assert_eq!(s.pane_output(&PaneId(1)).unwrap(), b"hello");
        assert_eq!(s.status(), BackendStatus::Connected);
    }

    #[test]
    fn state_change_variants_compile() {
        // 仅验证枚举可构造、可比较，不依赖运行时行为。
        let c1 = StateChange::WorkspaceRenamed {
            name: "main".into(),
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
            workspace_name: String::new(),
            workspace_runtime: "shell".into(),
            tabs: vec![],
            panes: vec![],
            layouts: vec![],
            outputs: vec![],
            status: BackendStatus::Disconnected,
            active_tab: None,
            active_pane: None,
        };
        assert!(s.active_tab().is_none());
        assert!(s.active_pane().is_none());
        assert!(s.layout(&TabId(1)).is_none());
        assert!(s.tab(&TabId(1)).is_none());
        assert!(s.tabs().is_empty());
        assert!(s.pane(&PaneId(1)).is_none());
        assert!(s.pane_output(&PaneId(1)).is_none());
        assert_eq!(s.status(), BackendStatus::Disconnected);
    }
}
