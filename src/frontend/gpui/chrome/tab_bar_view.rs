//! tab 栏投影：活动 tab、pane 数后缀、活动指示灯。
//!
//! 对照 macOS `UI/TabBar.swift`（位置 / 宽度策略 / 聚合视觉身份）与
//! `UI/StatusBarView.swift` 的 `TabActivityIndicatorView`（working 空心环、
//! blocked 粉、done 青、idle 不点亮）。

use std::collections::HashSet;

use crate::frontend::gpui::model::attention::{AttentionSnapshot, AttentionVisibilityKey};
use crate::frontend::gpui::model::sidebar::{self, Indicator, SidebarItem};
use crate::frontend::utils::corebridge::ClientTab;

/// tab 栏位置：来自 `[ui] tab_bar_position`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabBarPosition {
    #[default]
    Top,
    Bottom,
}

impl TabBarPosition {
    pub fn from_raw(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

/// tab 宽度策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabBarStyle {
    /// 平铺等宽。
    #[default]
    EqualWidth,
    /// 固定 128pt。
    Fixed,
    /// 按标题自然宽度，上限 280pt。
    Compact,
}

impl TabBarStyle {
    pub const FIXED_TAB_WIDTH: f32 = 128.0;
    pub const MAXIMUM_FITTED_TAB_WIDTH: f32 = 280.0;

    pub fn from_raw(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "equal_width" => Some(Self::EqualWidth),
            "fixed" => Some(Self::Fixed),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::EqualWidth => "equal_width",
            Self::Fixed => "fixed",
            Self::Compact => "compact",
        }
    }
}

/// 一个 tab 按钮要画的东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabBarItem {
    pub tab_id: u32,
    pub title: String,
    pub active: bool,
    /// 多 pane 时标题后缀 `(n)`。
    pub pane_count: usize,
    /// 活动指示灯；idle 不点亮，所以 `None`。
    pub activity: Option<Indicator>,
}

impl TabBarItem {
    /// 标题文本：活动 tab 与多 pane 布局都要一眼看出。
    pub fn label(&self) -> String {
        if self.pane_count > 1 {
            format!("{} ({})", self.title, self.pane_count)
        } else {
            self.title.clone()
        }
    }
}

/// 把当前 workspace 的 tab 列表投影成 tab 栏。
///
/// `workspaces` 是全部工作区（tab 活动来自它们的 attention），`tabs` 只取当前
/// 工作区的；两者分开是因为隐藏身份按 pane 记在任意工作区上。
pub fn build(
    workspace_id: &str,
    tabs: &[ClientTab],
    pane_counts: &std::collections::HashMap<u32, usize>,
    workspaces: &[SidebarItem],
    attention: Option<&AttentionSnapshot>,
    hidden: &HashSet<AttentionVisibilityKey>,
) -> Vec<TabBarItem> {
    let activities = sidebar::tab_activities(workspaces, attention, hidden);
    tabs.iter()
        .map(|tab| TabBarItem {
            tab_id: tab.id,
            title: tab.name.clone(),
            active: tab.is_active,
            pane_count: pane_counts.get(&tab.id).copied().unwrap_or(0),
            activity: activities
                .get(&sidebar::TabActivityKey {
                    workspace_id: workspace_id.to_string(),
                    tab_id: tab.id,
                })
                .copied(),
        })
        .collect()
}

/// 活动指示灯的视觉身份（圆点 / 空心环）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityMark {
    /// working：空心环，代表还在转。
    Ring,
    /// blocked：实心圆。
    Filled,
}

impl Indicator {
    /// tab 上怎么画；idle 返回 `None`（不点亮）。
    pub fn activity_mark(self) -> Option<ActivityMark> {
        match self {
            Indicator::Working => Some(ActivityMark::Ring),
            Indicator::Blocked | Indicator::Done => Some(ActivityMark::Filled),
            Indicator::Idle => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::model::attention::{
        PaneAttention, PaneAttentionStatus, WorkspaceAttention,
    };

    fn item(id: &str) -> SidebarItem {
        SidebarItem::new(id, "dev").with(|item| {
            item.runtime = "tmux".into();
            item.transport = "local".into();
            item.is_active = true;
        })
    }

    fn tab(id: u32, name: &str, active: bool) -> ClientTab {
        ClientTab {
            id,
            name: name.to_string(),
            is_active: active,
        }
    }

    fn snapshot(workspace_id: &str, panes: Vec<PaneAttention>) -> AttentionSnapshot {
        AttentionSnapshot {
            blocked_count: 0,
            workspaces: vec![WorkspaceAttention {
                workspace_id: workspace_id.to_string(),
                name: "dev".to_string(),
                transport: "local".to_string(),
                ..WorkspaceAttention::new(workspace_id.to_string(), panes)
            }],
        }
    }

    /// 对照 `TabBarStyle` 的字符串解析与常量。
    #[test]
    fn tab_bar_style_round_trips_and_keeps_the_widths() {
        for style in [
            TabBarStyle::EqualWidth,
            TabBarStyle::Fixed,
            TabBarStyle::Compact,
        ] {
            assert_eq!(TabBarStyle::from_raw(style.raw_value()), Some(style));
        }
        assert_eq!(TabBarStyle::from_raw("nonsense"), None);
        assert_eq!(TabBarStyle::FIXED_TAB_WIDTH, 128.0);
        assert_eq!(TabBarStyle::MAXIMUM_FITTED_TAB_WIDTH, 280.0);
    }

    /// 对照 `TabBarPosition`。
    #[test]
    fn tab_bar_position_round_trips() {
        assert_eq!(TabBarPosition::default(), TabBarPosition::Top);
        assert_eq!(
            TabBarPosition::from_raw("bottom"),
            Some(TabBarPosition::Bottom)
        );
        assert_eq!(TabBarPosition::from_raw(" TOP "), Some(TabBarPosition::Top));
        assert_eq!(TabBarPosition::from_raw(""), None);
    }

    /// 多 pane 加数量后缀，单 pane 不加。
    #[test]
    fn labels_only_add_the_count_for_split_tabs() {
        let item = TabBarItem {
            tab_id: 1,
            title: "work".into(),
            active: true,
            pane_count: 1,
            activity: None,
        };
        assert_eq!(item.label(), "work");
        let split = TabBarItem {
            pane_count: 3,
            ..item
        };
        assert_eq!(split.label(), "work (3)");
    }

    /// working 的 tab 亮灯，idle 的 tab 不亮。
    #[test]
    fn only_working_panes_light_the_tab() {
        let workspace_id = "local@@dev@tmux@dev";
        let workspaces = [item(workspace_id)];
        let tabs = [tab(1, "quiet", true), tab(2, "busy", false)];
        let mut pane_counts = std::collections::HashMap::new();
        pane_counts.insert(1, 1usize);
        pane_counts.insert(2, 2usize);
        let snapshot = snapshot(
            workspace_id,
            vec![PaneAttention {
                pane_id: 8,
                status: PaneAttentionStatus::Working,
                last_line: "cargo test".into(),
                seq: 2,
                ..PaneAttention::default()
            }],
        );
        // 没有 tab 归属的 pane 不点亮任何 tab。
        let items = build(
            workspace_id,
            &tabs,
            &pane_counts,
            &workspaces,
            Some(&snapshot),
            &HashSet::new(),
        );
        assert_eq!(items[0].activity, None);
        assert_eq!(items[1].activity, None);
        assert_eq!(items[1].label(), "busy (2)");
    }

    /// 指示灯视觉身份：working 空心环，其余实心。
    #[test]
    fn activity_marks_match_the_indicator_view() {
        assert_eq!(Indicator::Working.activity_mark(), Some(ActivityMark::Ring));
        assert_eq!(
            Indicator::Blocked.activity_mark(),
            Some(ActivityMark::Filled)
        );
        assert_eq!(Indicator::Done.activity_mark(), Some(ActivityMark::Filled));
        assert_eq!(Indicator::Idle.activity_mark(), None);
    }

    /// tab 归属已知时点亮对应 tab；同 tab 多 pane 取更高优先级。
    #[test]
    fn tabs_light_up_from_their_panes() {
        let workspace_id = "local@@dev@tmux@dev";
        let workspaces = [item(workspace_id).with(|item| {
            item.tab_id_by_pane.insert(9, 2);
            item.tab_id_by_pane.insert(10, 2);
        })];
        let tabs = [tab(1, "quiet", true), tab(2, "busy", false)];
        let mut pane_counts = std::collections::HashMap::new();
        pane_counts.insert(1, 1usize);
        pane_counts.insert(2, 2usize);
        let snapshot = snapshot(
            workspace_id,
            vec![
                PaneAttention {
                    pane_id: 9,
                    status: PaneAttentionStatus::Working,
                    process_name: Some("cargo".into()),
                    seq: 2,
                    ..PaneAttention::default()
                },
                PaneAttention {
                    pane_id: 10,
                    status: PaneAttentionStatus::Blocked,
                    process_name: Some("vim".into()),
                    seq: 3,
                    ..PaneAttention::default()
                },
            ],
        );
        let items = build(
            workspace_id,
            &tabs,
            &pane_counts,
            &workspaces,
            Some(&snapshot),
            &HashSet::new(),
        );
        assert_eq!(items[0].activity, None, "活动只在它所属的 tab 上");
        assert_eq!(items[1].activity, Some(Indicator::Blocked));
    }
}
