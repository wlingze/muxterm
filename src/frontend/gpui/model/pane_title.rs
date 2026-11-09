//! Pane 标题条：多 pane 时显示在终端上方，单 pane 隐藏。
//!
//! 对照 macOS `PaneTitleBarView` + `PaneTitleLayoutPolicy`：
//! - `showsTitleBar(visiblePaneCount:)` 只在可见 pane > 1 时为真；
//! - 标题条**不占**终端格子（`PaneTitleBarGeometry.reservedHeight` 恒为 0），
//!   它浮在 Surface 上方，所以显隐不改变 Core 的 cols/rows；
//! - 标题为空时显示 `Pane @<id>`，与 Swift 一致；
//! - 行内动作集固定：水平/垂直分割、全屏、循环布局、移动、关闭。

/// 标题条高度（Swift `PaneTitleBarGeometry.height`）。
pub const TITLE_BAR_HEIGHT: f32 = 22.0;

/// 标题条是否出现。
///
/// 对照 `PaneTitleLayoutPolicy.showsTitleBar(visiblePaneCount:)`。
pub fn shows_title_bar(visible_pane_count: usize) -> bool {
    visible_pane_count > 1
}

/// 标题条的保留高度：恒为 0，显隐不能改变终端格子。
///
/// 对照 `PaneTitleBarGeometry.reservedHeight(showsTitles:)`：Swift 里恒 0，
/// 因为标题条浮在 Surface 上而不是挤占它。
pub fn reserved_height(_shows_titles: bool) -> f32 {
    0.0
}

/// 标题文字。空白标题退回 `Pane @<id>`。
pub fn display_title(title: &str, pane_id: u32) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        format!("Pane @{pane_id}")
    } else {
        trimmed.to_string()
    }
}

/// 标题条右侧动作（顺序与 Swift `actionsForTesting` 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneTitleAction {
    SplitHorizontal,
    SplitVertical,
    Fullscreen,
    CycleLayout,
    Close,
}

impl PaneTitleAction {
    /// 菜单里的文案（英文，与 Swift `i18n.tr(.menuSplitHorizontal)` 等对应）。
    pub fn label(self) -> &'static str {
        match self {
            Self::SplitHorizontal => "Split Horizontal",
            Self::SplitVertical => "Split Vertical",
            Self::Fullscreen => "Toggle Pane Fullscreen",
            Self::CycleLayout => "Cycle Layout",
            Self::Close => "Close Pane",
        }
    }

    /// 动作顺序固定，`⋯` 菜单直接照它铺。
    pub const ALL: [Self; 5] = [
        Self::SplitHorizontal,
        Self::SplitVertical,
        Self::Fullscreen,
        Self::CycleLayout,
        Self::Close,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `FlatChromeTests.testPaneTitleBarOnlyForMultiplePanes`。
    #[test]
    fn title_bar_only_shows_when_split() {
        assert!(!shows_title_bar(1));
        assert!(shows_title_bar(2));
        assert!(shows_title_bar(4));
    }

    /// 对照 `BreakPaneE2ETests`：显隐都不占终端高度。
    #[test]
    fn title_bar_never_reserves_terminal_rows() {
        assert_eq!(reserved_height(false), 0.0);
        assert_eq!(reserved_height(true), 0.0);
    }

    #[test]
    fn empty_titles_fall_back_to_the_pane_id() {
        assert_eq!(display_title("", 7), "Pane @7");
        assert_eq!(display_title("   ", 7), "Pane @7");
        assert_eq!(display_title(" zsh ", 7), "zsh");
    }

    #[test]
    fn actions_match_the_swift_menu_order() {
        assert_eq!(
            PaneTitleAction::ALL.map(|action| action.label()),
            [
                "Split Horizontal",
                "Split Vertical",
                "Toggle Pane Fullscreen",
                "Cycle Layout",
                "Close Pane",
            ]
        );
    }
}
