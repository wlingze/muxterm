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
    ///
    /// 宽度策略来自 `[ui] tab_bar_style`：缺省 `equal_width` 时每个 tab 平分
    /// 可用宽度并铺满整条带子（对照 Swift `tabStack.distribution = .fillEqually`
    /// 与 `tabStackTrailingConstraint.isActive`）；`fixed` / `compact` 保持自然宽度。
    pub(crate) fn render_tabs(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        use crate::frontend::gpui::chrome::tab_bar_view::TabBarStyle;
        let theme = self.theme().clone();
        let items = self.tab_items();
        let count = items.len();
        let style = self.tab_bar_style();
        let viewport = self.tab_strip_width();
        let width = style.tab_width(viewport, count);
        let equal = style == TabBarStyle::EqualWidth && count > 0;
        let tabs: Vec<AnyElement> = items
            .into_iter()
            .map(|item| render_tab(item, &theme, cx, width, equal))
            .collect();
        div()
            .id("tab-strip")
            .debug_selector(|| "tab-strip".to_string())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(if equal { 2.0 } else { 3.0 }))
            // 等宽样式必须把整条带子占满：`flex_1` 让它吃掉中间所有空间，
            // 每个 tab 再 `flex_1` 平分；fixed/compact 用自然宽度。
            .when(equal, |element| element.flex_1().min_w(px(0.0)))
            .children(tabs)
            .into_any_element()
    }
}

/// tab 的状态色：working / blocked / done 直接用状态色，idle 用 muted。
///
/// 用户报障（截图 3）：tab 上看不出「在跑 / 跑完了」。之前只有一个小环，
/// 现在把状态色同时用到标题与底部下划线上，一眼就能分清楚。
pub fn tab_state_color(item: &TabBarItem, theme: &Theme) -> Rgba {
    match item.activity {
        Some(Indicator::Working) => theme.working,
        Some(Indicator::Blocked) => theme.blocked,
        Some(Indicator::Done) => theme.done,
        _ => theme.muted,
    }
}

pub(crate) fn render_tab(
    item: TabBarItem,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
    width: Option<f32>,
    equal: bool,
) -> AnyElement {
    let tab_id = item.tab_id;
    let active = item.active;
    let mark = item.activity.and_then(Indicator::activity_mark);
    // 状态色：working = systemYellow、blocked = systemPink、done = systemTeal
    // （对照 `TabActivityIndicatorView`）。idle 用 muted。
    let activity_color = tab_state_color(&item, theme);

    // 聚合页的 tab 用槽的身份色（Shells 青 / Agents 紫），普通 tab 用强调色。
    let accent = match item.identity {
        Some(identity) => crate::frontend::gpui::chrome::sidebar::identity_color(identity, theme),
        None => theme.accent,
    };
    // 活动 tab：强调色 14% 底 + 2pt 下划线；非活动：透明底。
    let background = if active {
        Rgba { a: 0.14, ..accent }
    } else {
        Rgba { a: 0.0, ..accent }
    };
    // 用户报障（截图 3）：tab 看不出运行/完成状态。有状态时标题就用状态色，
    // 非活动的活跃 tab 也看得见颜色；idle 仍按原来的 label/muted 走。
    let label_color = match item.activity {
        Some(_) => activity_color,
        None if active => theme.label(),
        None => theme.muted,
    };

    let mut children: Vec<AnyElement> = Vec::new();
    if let Some(mark) = mark {
        // 活动标记也是矢量图形：环形 / 实心点，不用字体字形（`◌` 在
        // fallback 字体里会变成方块或跳基线）。
        let icon = match mark {
            ActivityMark::Ring => crate::frontend::gpui::chrome::icons::Icon::Circle,
            ActivityMark::Filled => crate::frontend::gpui::chrome::icons::Icon::CircleFilled,
        };
        children.push(crate::frontend::gpui::chrome::icons::marker_icon(
            icon,
            activity_color,
        ));
    }
    children.push(
        div()
            .debug_selector(move || format!("tab-label-{tab_id}"))
            .min_w_0()
            // 长 tab 标题（重命名、命令名）必须单行 + 省略号：换行会把
            // 标题撑成两行，把关闭按钮顶出 tab、压缩到只剩半个字。
            .truncate()
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
            .child(crate::frontend::gpui::chrome::icons::trailing_icon(
                crate::frontend::gpui::chrome::icons::Icon::Xmark,
                theme.muted,
            ))
            .into_any_element()
    });

    let debug_id = format!("tab-{tab_id}");
    div()
        .id(SharedString::from(format!("tab-{tab_id}")))
        // 无头测试要按 tab id 查边界（等宽铺满 / 状态色都靠它断言）。
        .debug_selector(move || debug_id)
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
        // 等宽样式：每个 tab 平分整条带子；fixed/compact 用固定宽度。
        .when(equal, |element| element.flex_1().min_w(px(0.0)))
        .when_some(width.filter(|_| !equal), |element, width| {
            element.w(px(width)).flex_none()
        })
        // 底部状态线：active 用强调色（或聚合身份色），有活动状态时用状态色，
        // 这样没被选中的 tab 也能看出「在跑 / 跑完了」。
        .when(item.activity.is_some() || active, |element| {
            let line = if item.activity.is_some() {
                activity_color
            } else {
                accent
            };
            element.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(ACTIVE_UNDERLINE))
                    .bg(line),
            )
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| this.select_tab(tab_id, cx)),
        )
        .children(children)
        .when_some(close_button, |element, button| element.child(button))
        .into_any_element()
}
