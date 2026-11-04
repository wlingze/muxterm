//! tab 栏渲染：活动 tab、数量后缀、活动指示灯、点击切换。
//!
//! 行内容来自 `tab_bar_view::build`（纯投影）。视觉规则对照
//! `UI/StatusBarView.swift` 的 `TabActivityIndicatorView`：working 空心环、
//! blocked/done 实心点、idle 不点亮。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::chrome::tab_bar_view::{ActivityMark, TabBarItem};
use crate::frontend::gpui::model::sidebar::Indicator;
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::Theme;

const TAB_BAR_HEIGHT: f32 = 30.0;

impl GpuiApp {
    pub(crate) fn render_tab_bar(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme().clone();
        let tabs: Vec<AnyElement> = self
            .tab_items()
            .into_iter()
            .map(|item| render_tab(item, &theme, cx))
            .collect();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(TAB_BAR_HEIGHT))
            .px_2()
            .overflow_hidden()
            .bg(theme.chrome_bg)
            .border_color(theme.border)
            .when(
                self.tab_bar_position()
                    == crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Top,
                |element| element.border_b_1(),
            )
            .when(
                self.tab_bar_position()
                    == crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Bottom,
                |element| element.border_t_1(),
            )
            .children(tabs)
            .into_any_element()
    }
}

fn render_tab(item: TabBarItem, theme: &Theme, cx: &mut gpui::Context<GpuiApp>) -> AnyElement {
    let tab_id = item.tab_id;
    let mark = item.activity.and_then(Indicator::activity_mark);
    let activity_color = match item.activity {
        Some(Indicator::Working) => theme.warning,
        Some(Indicator::Blocked) => theme.danger,
        Some(Indicator::Done) => theme.success,
        _ => theme.muted,
    };

    let mut children: Vec<AnyElement> = Vec::new();
    if let Some(mark) = mark {
        // working 空心环、blocked/done 实心点：与 `TabActivityIndicatorView` 一致。
        children.push(
            div()
                .flex_none()
                .text_xs()
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
            .child(SharedString::from(item.label()))
            .into_any_element(),
    );

    div()
        .id(SharedString::from(format!("tab-{tab_id}")))
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_3()
        .h_full()
        .max_w(px(240.0))
        .cursor_pointer()
        .rounded_sm()
        .text_sm()
        .bg(if item.active {
            theme.bg
        } else {
            theme.chrome_bg
        })
        .text_color(if item.active { theme.fg } else { theme.muted })
        .border_1()
        .border_color(if item.active {
            theme.accent
        } else {
            theme.chrome_bg
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| this.select_tab(tab_id, cx)),
        )
        .children(children)
        .into_any_element()
}
