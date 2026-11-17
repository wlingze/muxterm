//! 侧栏渲染：四段（Workspaces / Agents / Commands / Hidden Commands）。
//!
//! 行内容全部来自 `sidebar_view::build`（纯投影，带单测）；这里只负责铺像素和
//! 接点击。对照 macOS `App/WorkspaceSidebar.swift`：36pt 行、段头可折叠、
//! 命令行尾部按钮切换可见性、可关闭工作区尾部一个关闭按钮。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, MouseMoveEvent, SharedString};

use crate::frontend::gpui::chrome::icons::{self, Icon};
use crate::frontend::gpui::chrome::sidebar_view::{
    CommandKey, RowTarget, Section, SidebarRow, SidebarSectionModel,
};
use crate::frontend::gpui::model::sidebar::Indicator;
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::Theme;

/// 与 Swift `table.rowHeight` 一致。
const ROW_HEIGHT: f32 = 36.0;
/// 侧栏宽度：与 macOS `mainSplitController.setPosition(240)` 一致。
pub const SIDEBAR_WIDTH: f32 = 240.0;
/// 段头高度：Swift 的 `NSButton` 段头（26pt）+ 上下留白。
const SECTION_HEIGHT: f32 = 26.0;
/// 段头左右内边距：与 Swift `header.leading = 6` 一致。
const SECTION_INSET: f32 = 6.0;
/// 行左右内边距：Swift cell 的 marker 从 9pt 起（`leadingAnchor + 9`）。
const ROW_INSET: f32 = 9.0;
/// 行间距：Swift `table.intercellSpacing.height = 1`。
const ROW_SPACING: f32 = 1.0;
/// marker 列宽（Swift `marker.width = 11`）。
const MARKER_WIDTH: f32 = 11.0;
/// marker 与编号之间的间距（Swift `shortcutLabel.leading = marker.trailing + 6`）。
const MARKER_TO_SHORTCUT: f32 = 6.0;
/// 编号列宽（Swift `shortcutLabel.width = 13`）。
const SHORTCUT_WIDTH: f32 = 13.0;
/// 编号与标题之间的间距（Swift `titleLabel.leading = shortcut.trailing + 3`）。
const SHORTCUT_TO_TITLE: f32 = 3.0;
/// 尾部按钮宽（Swift `trailingButtonWidth = 22`）。
const TRAILING_WIDTH: f32 = 22.0;

impl GpuiApp {
    pub(crate) fn render_sidebar(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        use crate::frontend::gpui::model::sidebar_layout::{
            clamp_sidebar_width, SidebarSectionHeights, SECTION_COLLAPSED_HEIGHT,
            SIDEBAR_DIVIDER_HIT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
        };
        let theme = self.theme().clone();
        let width = clamp_sidebar_width(self.sidebar_width, self.window_width, 320.0);
        let models = self.sidebar_model();
        let find = |kind: Section| models.iter().find(|model| model.section == kind).cloned();
        let workspaces = find(Section::Workspaces);
        let agents = find(Section::Agents);
        let commands = find(Section::Commands);
        let hidden = find(Section::HiddenCommands);

        // 下半区每段至少留一个段头；Hidden Commands 固定贴底，只在展开时
        // 按行高增加高度。
        let hidden_rows = hidden
            .as_ref()
            .filter(|model| model.expanded)
            .map(|model| model.rows.len())
            .unwrap_or(0) as f32;
        let hidden_height = SECTION_COLLAPSED_HEIGHT + hidden_rows * ROW_HEIGHT;
        let available = (self.window_height - 8.0).max(0.0);
        let heights = SidebarSectionHeights::compute(available, hidden_height);

        // 三段各自滚动：workspace 再多也挤不掉下面两个区，反之亦然。
        let make = |model: Option<SidebarSectionModel>,
                    height: Option<f32>,
                    fill: bool,
                    cx: &mut gpui::Context<Self>| {
            let Some(model) = model else {
                return div().into_any_element();
            };
            let kind = model.section;
            let body = self.render_section(model, &theme, cx);
            let mut column = div().flex().flex_col().flex_none().w_full();
            if let Some(height) = height {
                column = column.h(px(height));
            }
            if fill {
                column = column.flex_1().min_h(px(0.0));
            }
            column
                .child(
                    div()
                        .id(SharedString::from(format!("sidebar-scroll-{:?}", kind)))
                        .debug_selector(move || format!("sidebar-scroll-{kind:?}"))
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .child(body),
                )
                .into_any_element()
        };

        let (workspaces, agents, commands) = match heights {
            Some(heights) => (
                make(workspaces, Some(heights.workspaces), false, cx),
                make(agents, Some(heights.agents), false, cx),
                make(commands, Some(heights.commands), false, cx),
            ),
            // 高度不够：只让 Workspaces 吃掉剩下的空间，不硬排三区。
            None => (
                make(workspaces, None, true, cx),
                make(agents, None, false, cx),
                make(commands, None, false, cx),
            ),
        };

        let column = div()
            .flex()
            .flex_col()
            .w(px(width))
            .debug_selector(|| "sidebar".to_string())
            .h_full()
            .overflow_hidden()
            .py(px(4.0))
            .bg(theme.sidebar_bg)
            .border_color(theme.border)
            .child(workspaces)
            .child(agents)
            .child(commands)
            // Hidden Commands 固定在整栏最底部：Swift 里它最后一段、默认收起。
            .child(
                div()
                    .flex_none()
                    .w_full()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(self.render_section_plain(
                        hidden.unwrap_or_else(|| SidebarSectionModel {
                            section: Section::HiddenCommands,
                            title: Section::HiddenCommands.title(),
                            expanded: false,
                            rows: Vec::new(),
                        }),
                        &theme,
                        cx,
                    )),
            );

        // 拖动宽度：热区是右侧一条 5pt 的透明带（视觉上仍是 1pt 边线）。
        div()
            .relative()
            .flex_none()
            .h_full()
            .child(column)
            .child(
                div()
                    .id("sidebar-divider")
                    .debug_selector(|| "sidebar-divider".to_string())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(-SIDEBAR_DIVIDER_HIT_WIDTH / 2.0))
                    .w(px(SIDEBAR_DIVIDER_HIT_WIDTH))
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    // 拖动时鼠标会离开这条 5pt 热区，所以除了 `on_mouse_down`
                    // 还要在同一个元素上挂 `on_drag_move`：gpui 会把后续 move
                    // 继续交给按下时命中的元素，热区不会「拖丢」。
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.sidebar_dragging = true;
                            this.sidebar_drag_offset =
                                f32::from(event.position.x) - this.sidebar_width;
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                        if !this.sidebar_dragging || event.pressed_button.is_none() {
                            return;
                        }
                        let target = f32::from(event.position.x) - this.sidebar_drag_offset;
                        let next = clamp_sidebar_width(target, this.window_width, 320.0)
                            .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
                        if (next - this.sidebar_width).abs() > 0.5 {
                            this.sidebar_width = next;
                            cx.notify();
                        }
                    })),
            )
            .into_any_element()
    }

    /// 拖动期间的全局遮罩：鼠标一旦离开分隔条热区，分隔条自己就收不到 move 事件了，
    /// 所以拖动时铺一层满窗口的透明层接管 move / up。
    pub(crate) fn render_sidebar_drag_overlay(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        use crate::frontend::gpui::model::sidebar_layout::{
            clamp_sidebar_width, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
        };
        if !self.sidebar_dragging {
            return div().into_any_element();
        }
        div()
            .id("sidebar-drag-overlay")
            .debug_selector(|| "sidebar-drag-overlay".to_string())
            .absolute()
            .left_0()
            .top_0()
            .right_0()
            .bottom_0()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if !this.sidebar_dragging {
                    return;
                }
                let target = f32::from(event.position.x) - this.sidebar_drag_offset;
                let next = clamp_sidebar_width(target, this.window_width, 320.0)
                    .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
                if (next - this.sidebar_width).abs() > 0.5 {
                    this.sidebar_width = next;
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    this.sidebar_dragging = false;
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// 段渲染（不含外层滚动区）：三区已各自带滚动。
    fn render_section_plain(
        &self,
        section: SidebarSectionModel,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        self.render_section(section, theme, cx)
    }

    fn render_section(
        &self,
        section: SidebarSectionModel,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let kind = section.section;
        let header_title = section_header_title(&section);
        // 空段只留段头：Swift 的表格段没有行时不占高度，但段头仍然可点。
        let mut children: Vec<AnyElement> = Vec::new();
        if section.expanded {
            children.extend(
                section
                    .rows
                    .into_iter()
                    .map(|row| self.render_sidebar_row(kind, row, theme, cx)),
            );
        }

        div()
            .flex()
            .flex_col()
            .when(kind != Section::Workspaces, |element| {
                // 段间隔一条 hairline + 8pt：Swift 的四个独立表格之间就是
                // 这种「分组卡片」的边界，没有分隔线会糊成一大片。
                element
                    .pt(px(8.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .mt(px(4.0))
                    .mb(px(4.0))
            })
            .child(
                div()
                    .id(SharedString::from(format!("section-{:?}", kind)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(SECTION_HEIGHT))
                    .px(px(SECTION_INSET))
                    .cursor_pointer()
                    .text_size(px(crate::frontend::gpui::theme::SIDEBAR_SECTION_FONT_SIZE))
                    .line_height(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.muted)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.toggle_sidebar_section(kind, cx)),
                    )
                    // 对照 Swift `configureHeader`：chevron 在前、标题在后。
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .child(icons::chevron_icon(section.expanded, theme.muted))
                            .child(SharedString::from(header_title)),
                    )
                    .child(div().flex().flex_row().items_center().gap(px(2.0)).when(
                        kind == Section::Workspaces,
                        |element| {
                            // 对照 `WorkspaceSidebar` 的 `addWorkspaceButton`：新建 project。
                            element.child(
                                div()
                                    .id("sidebar-add-workspace")
                                    .debug_selector(|| "sidebar-add-workspace".to_string())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme.active_bg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.open_target_editor(None, cx);
                                        }),
                                    )
                                    .child(icons::plus_icon(theme.muted)),
                            )
                        },
                    )),
            )
            .children(children)
            .into_any_element()
    }

    fn render_sidebar_row(
        &self,
        section: Section,
        row: SidebarRow,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        // 聚合槽是固定入口，用专属身份色；普通行才跟连接/活动状态走。
        let marker_color = match row.identity {
            Some(identity) => identity_color(identity, theme),
            None => indicator_color(row.indicator, theme),
        };
        let detail_color = if row.detail_alert {
            theme.danger
        } else {
            theme.muted
        };
        let target = row.target.clone();
        let shortcut = row.shortcut.clone();
        let trailing = row.trailing;
        let command_key = match (&row.target, section) {
            (
                RowTarget::Pane {
                    workspace_id,
                    pane_id,
                    ..
                },
                Section::Commands | Section::HiddenCommands,
            ) => Some(CommandKey {
                workspace_id: workspace_id.clone(),
                pane_id: *pane_id,
            }),
            _ => None,
        };

        // Swift 的尾部按钮（runtime 更新 / 关闭）只在 hover 时才出现
        // （`trailingShowsOnHover`）；一直挂着会把每行都画得很吵。
        let hover_group = SharedString::from(format!("sidebar-row-{:?}-{}", section, row.title));
        let mut trailing_buttons: Vec<AnyElement> = Vec::new();
        if let Some(symbol) = trailing {
            // 矢量图标，不用 emoji：`👁` / `🚫` 是彩色字形，字号一变就跳基线。
            let icon = icons::icon_for_trailing(symbol);
            let close_target = if symbol == "xmark" {
                match &row.target {
                    RowTarget::Workspace(id) => Some(id.clone()),
                    _ => None,
                }
            } else {
                None
            };
            trailing_buttons.push(
                div()
                    .id(SharedString::from(format!(
                        "trailing-{:?}-{}-{}",
                        section, row.title, symbol
                    )))
                    .debug_selector({
                        let title = row.title.clone();
                        move || format!("sidebar-trailing-{title}")
                    })
                    .group_hover(hover_group.clone(), |style| style.opacity(1.0))
                    .opacity(if row.active { 1.0 } else { 0.0 })
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(TRAILING_WIDTH))
                    .h(px(18.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.active_bg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            // 尾部按钮不触发整行跳转。
                            cx.stop_propagation();
                            if let Some(key) = command_key.clone() {
                                this.toggle_command_visibility(key, cx);
                            } else if let Some(id) = close_target.clone() {
                                this.close_workspace(id, cx);
                            }
                        }),
                    )
                    .child(icons::trailing_icon(icon, theme.muted))
                    .into_any_element(),
            );
        }

        // 行的视觉：Swift sourceList 的单行卡片 —— 圆角选中底、悬停淡底，
        // 左侧 2pt 身份色竖条标出「当前」这一行（比整行高亮更克制）。
        let accent_strip = if row.active { Some(marker_color) } else { None };
        let marker_icon_element = match row.identity {
            Some(_) => icons::marker_icon(Icon::Diamond, marker_color),
            None => icons::indicator_glyph(row.indicator, marker_color),
        };

        div()
            .id(SharedString::from(format!(
                "row-{:?}-{}",
                section, row.title
            )))
            .group(hover_group.clone())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_HEIGHT - ROW_SPACING))
            .mb(px(ROW_SPACING))
            .mx(px(6.0))
            .rounded(px(5.0))
            .overflow_hidden()
            .cursor_pointer()
            .bg(if row.active {
                theme.active_bg
            } else {
                theme.sidebar_bg
            })
            .hover(|style| style.bg(theme.active_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus_handle());
                    this.activate_sidebar_row(target.clone(), cx);
                }),
            )
            .when_some(accent_strip, |element, color| {
                // 2pt 竖条：贴左边、比行短一点，像 sourceList 的强调线。
                element.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(6.0))
                        .bottom(px(6.0))
                        .w(px(2.0))
                        .rounded(px(1.0))
                        .bg(color),
                )
            })
            .child(
                // 标记列固定 11pt（Swift `marker.width = 11`），行与行才对得齐。
                div()
                    .flex_none()
                    .ml(px(ROW_INSET))
                    .w(px(MARKER_WIDTH))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(marker_icon_element),
            )
            // 编号 / 字母紧跟标记列（Swift `shortcutLabel` 在 marker 右侧 6pt，
            // 宽 13pt 居中）。放在行尾会让数字离标题很远，扫视时对不上。
            .when_some(shortcut, |element, shortcut| {
                element.child(
                    div()
                        .debug_selector({
                            let title = row.title.clone();
                            move || format!("sidebar-shortcut-{title}")
                        })
                        .flex_none()
                        .ml(px(MARKER_TO_SHORTCUT))
                        .w(px(SHORTCUT_WIDTH))
                        .flex()
                        .justify_center()
                        .text_size(px(crate::frontend::gpui::theme::SIDEBAR_SHORTCUT_FONT_SIZE))
                        .line_height(px(11.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(if row.active {
                            theme.label()
                        } else {
                            theme.muted
                        })
                        .child(SharedString::from(shortcut)),
                )
            })
            .child(
                div()
                    .debug_selector({
                        let title = row.title.clone();
                        move || format!("sidebar-title-{title}")
                    })
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .ml(px(SHORTCUT_TO_TITLE))
                    .overflow_hidden()
                    .child(
                        div()
                            .debug_selector({
                                let title = row.title.clone();
                                move || format!("sidebar-name-{title}")
                            })
                            .text_size(px(crate::frontend::gpui::theme::SIDEBAR_TITLE_FONT_SIZE))
                            .line_height(px(15.0))
                            .font_weight(if row.emphasized || row.active {
                                gpui::FontWeight::SEMIBOLD
                            } else {
                                gpui::FontWeight::MEDIUM
                            })
                            .text_color(theme.label())
                            // 长名字必须单行 + 省略号：换行会把行撑成两行，
                            // 压住副标题、还把尾部关闭按钮顶出可视区
                            // （Swift 的 `NSTextField` 是 byTruncatingTail）。
                            .truncate()
                            .child(SharedString::from(row.title.clone())),
                    )
                    .child(
                        div()
                            .debug_selector({
                                let title = row.title.clone();
                                move || format!("sidebar-detail-{title}")
                            })
                            .text_size(px(crate::frontend::gpui::theme::SIDEBAR_DETAIL_FONT_SIZE))
                            .line_height(px(12.0))
                            .text_color(detail_color)
                            .truncate()
                            .child(SharedString::from(row.detail)),
                    ),
            )
            .children(trailing_buttons)
            .into_any_element()
    }
}

/// 段头文案：`WORKSPACES 6`。
///
/// 对照 macOS `WorkspaceSidebar` 的段头：有行时才带计数，空段只留标题。
fn section_header_title(section: &SidebarSectionModel) -> String {
    if section.rows.is_empty() {
        section.title.clone()
    } else {
        format!("{}  {}", section.title, section.rows.len())
    }
}

/// 指示标记的颜色：done 用成功色、blocked 用危险色、working 用强调色。
/// 聚合槽身份色：Shells = teal、Agents = purple（不用状态色，避免误读）。
pub(crate) fn identity_color(
    identity: crate::frontend::gpui::model::aggregate::AggregateIdentity,
    theme: &Theme,
) -> gpui::Rgba {
    use crate::frontend::gpui::model::aggregate::AggregateIdentity;
    match identity {
        AggregateIdentity::Shells => theme.done,
        AggregateIdentity::Agents => theme.purple,
    }
}

fn indicator_color(indicator: Indicator, theme: &Theme) -> gpui::Rgba {
    // 与 macOS `WorkspaceSidebar.indicatorColor` 完全一致：
    // working = systemYellow、blocked = systemPink、done = systemTeal。
    match indicator {
        Indicator::Done => theme.done,
        Indicator::Blocked => theme.blocked,
        Indicator::Working => theme.working,
        Indicator::Idle => theme.muted,
    }
}
