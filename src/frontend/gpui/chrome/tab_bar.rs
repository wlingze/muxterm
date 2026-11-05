//! tab 栏渲染：活动 tab、数量后缀、活动指示灯、点击切换。
//!
//! 行内容来自 `tab_bar_view::build`（纯投影）。视觉对照 macOS
//! `UI/StatusBarView.swift`：
//! - 栏高 24pt（`FlatChrome.tabBarHeight`），左右内边距 6pt；
//! - 一个 tab 是圆角 4、左右内边距 8 的小块，活动 tab 用强调色 14% 底 +
//!   2pt 下划线，非活动 tab 无底色；
//! - 标题是 `1  zsh`（`StatusBarTabTitle.display`），活动 tab 用 semibold；
//! - working 空心环 / blocked·done 实心点（`TabActivityIndicatorView`）。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, Rgba, SharedString};

use crate::frontend::gpui::chrome::tab_bar_view::{ActivityMark, TabBarItem};
use crate::frontend::gpui::model::sidebar::Indicator;
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::{Theme, TAB_BAR_FONT_SIZE};

/// `FlatChrome.tabBarHeight`。
pub const TAB_BAR_HEIGHT: f32 = 24.0;
/// `FlatChrome.statusHorizontalInset`。
pub(crate) const TAB_BAR_INSET: f32 = 6.0;
/// `FlatChrome.tabCellHorizontalInset`。
const TAB_CELL_INSET: f32 = 8.0;
/// `FlatChrome.activeTabUnderlineHeight`。
const ACTIVE_UNDERLINE: f32 = 2.0;

impl GpuiApp {
    /// 当前 workspace 的 tab 单元。tab 栏本身由状态栏组合（Swift 只有一条 bar）。
    pub(crate) fn render_tabs(&self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme().clone();
        self.tab_items()
            .into_iter()
            .map(|item| render_tab(item, &theme, cx))
            .collect()
    }
}

pub(crate) fn render_tab(
    item: TabBarItem,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let tab_id = item.tab_id;
    let active = item.active;
    let mark = item.activity.and_then(Indicator::activity_mark);
    let activity_color = match item.activity {
        Some(Indicator::Working) => theme.warning,
        Some(Indicator::Blocked) => theme.danger,
        Some(Indicator::Done) => theme.success,
        _ => theme.muted,
    };

    // 活动 tab：强调色 14% 底 + 2pt 下划线；非活动：透明底。
    let background = if active {
        Rgba {
            a: 0.14,
            ..theme.accent
        }
    } else {
        Rgba {
            a: 0.0,
            ..theme.accent
        }
    };
    let label_color = if active { theme.label() } else { theme.muted };

    let mut children: Vec<AnyElement> = Vec::new();
    if let Some(mark) = mark {
        children.push(
            div()
                .flex_none()
                .text_size(px(TAB_BAR_FONT_SIZE))
                .text_color(activity_color)
                .child(SharedString::from(match mark {
                    ActivityMark::Ring => "◌",
                    ActivityMark::Filled => "●",
                }))
                .into_any_element(),
        );
    }
    children.push(
        div()
            .overflow_hidden()
            .text_size(px(TAB_BAR_FONT_SIZE))
            .line_height(px(14.0))
            .text_color(label_color)
            .when(active, |element| {
                element.font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .child(SharedString::from(item.label()))
            .into_any_element(),
    );

    let close_button = active.then(|| {
        div()
            .id(SharedString::from(format!("tab-close-{tab_id}")))
            .flex_none()
            .px(px(2.0))
            .text_size(px(TAB_BAR_FONT_SIZE - 1.0))
            .text_color(theme.muted)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    // 关 tab 不触发切换，先吃掉这次点击。
                    cx.stop_propagation();
                    this.close_tab(tab_id, cx);
                }),
            )
            .child(SharedString::from("✕"))
            .into_any_element()
    });

    div()
        .id(SharedString::from(format!("tab-{tab_id}")))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .h(px(TAB_BAR_HEIGHT - 6.0))
        .px(px(TAB_CELL_INSET))
        .overflow_hidden()
        .rounded(px(4.0))
        .bg(background)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| this.select_tab(tab_id, cx)),
        )
        .children(children)
        .when_some(close_button, |element, button| element.child(button))
        .when(active, |element| {
            element.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(ACTIVE_UNDERLINE))
                    .bg(theme.accent),
            )
        })
        .into_any_element()
}
