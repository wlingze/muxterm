//! 侧栏分段：默认 Workspaces 占一半，Agents 与 Commands 各占四分之一。
//! 折叠段头归到底部，展开段有独立滚动正文和可拖动分隔条。

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
pub const SIDEBAR_WIDTH: f32 = crate::frontend::gpui::model::sidebar_layout::SIDEBAR_WIDTH;
/// 段头高度：Swift 的 `NSButton` 段头（26pt）。
const SECTION_HEIGHT: f32 =
    crate::frontend::gpui::model::sidebar_layout::SIDEBAR_SECTION_HEADER_HEIGHT;
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
            clamp_sidebar_width, section_heights, DEFAULT_SECTION_WEIGHTS,
            SIDEBAR_DIVIDER_HIT_WIDTH, SIDEBAR_VERTICAL_INSET,
        };
        let theme = self.theme().clone();
        let width = clamp_sidebar_width(self.sidebar_width, self.window_width, 320.0);
        let mut by_section: Vec<Option<SidebarSectionModel>> = vec![None; Section::ALL.len()];
        for model in self.sidebar_model() {
            let index = section_index(model.section);
            by_section[index] = Some(model);
        }
        if by_section[section_index(Section::HiddenCommands)].is_none() {
            by_section[section_index(Section::HiddenCommands)] = Some(SidebarSectionModel {
                section: Section::HiddenCommands,
                title: Section::HiddenCommands.title(),
                expanded: false,
                rows: Vec::new(),
            });
        }

        let expanded = Section::ALL.map(|section| self.sidebar_expansion.is_expanded(section));
        // 侧栏在顶部 chrome 下面（非 macOS 有自绘菜单条，CSD 平台还有标题栏），
        // 可分配高度要扣掉它们，否则段高比容器多出一整条，底部被裁掉。
        let content_height =
            (self.window_height - self.top_chrome_height() - SIDEBAR_VERTICAL_INSET * 2.0).max(0.0);
        let weights = if self
            .sidebar_section_weights
            .iter()
            .all(|weight| *weight == 0.0)
        {
            DEFAULT_SECTION_WEIGHTS
        } else {
            self.sidebar_section_weights
        };
        let heights = section_heights(content_height, expanded, weights);
        let mut expanded_sections = Vec::new();
        let mut collapsed_sections = Vec::new();
        let mut dividers = Vec::new();
        let mut boundary = 0.0;
        let mut previous_expanded = None;
        for section in Section::ALL {
            let index = section_index(section);
            let model = by_section[index]
                .take()
                .unwrap_or_else(|| SidebarSectionModel {
                    section,
                    title: section.title(),
                    expanded: false,
                    rows: Vec::new(),
                });
            if expanded[index] {
                if let Some(previous) = previous_expanded {
                    dividers.push(self.render_split_divider(
                        previous,
                        section,
                        boundary,
                        theme.border,
                        cx,
                    ));
                }
                expanded_sections.push(self.render_section(model, heights[index], &theme, cx));
                boundary += heights[index];
                previous_expanded = Some(section);
            } else {
                collapsed_sections.push(self.render_section(model, heights[index], &theme, cx));
            }
        }

        let mut section_column = div()
            .id("sidebar-sections")
            .debug_selector(|| "sidebar-sections".to_string())
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .min_h(px(0.0));
        if expanded_sections.is_empty() {
            section_column = section_column.child(div().flex_1().min_h(px(0.0)));
        }
        section_column = section_column.children(expanded_sections);
        section_column = section_column.children(collapsed_sections);
        section_column = section_column.children(dividers);

        let column = div()
            .flex()
            .flex_col()
            .w(px(width))
            .debug_selector(|| "sidebar".to_string())
            .h_full()
            .overflow_hidden()
            .py(px(SIDEBAR_VERTICAL_INSET))
            .bg(theme.sidebar_bg)
            .border_color(theme.border)
            .child(section_column);

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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.sidebar_dragging = true;
                            this.sidebar_split_drag = None;
                            this.sidebar_drag_offset =
                                f32::from(event.position.x) - this.sidebar_width;
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }

    fn render_split_divider(
        &self,
        upper: Section,
        lower: Section,
        boundary: f32,
        border: gpui::Rgba,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        use crate::frontend::gpui::model::sidebar_layout::{
            section_heights, SidebarSplitDrag, SIDEBAR_DIVIDER_HIT_WIDTH, SIDEBAR_VERTICAL_INSET,
        };
        let split = SIDEBAR_DIVIDER_HIT_WIDTH;
        div()
            .id(SharedString::from(format!(
                "sidebar-split-{upper:?}-{lower:?}"
            )))
            .debug_selector(move || format!("sidebar-split-{upper:?}-{lower:?}"))
            .absolute()
            .left_0()
            .right_0()
            .top(px(boundary - split / 2.0))
            .h(px(split))
            .flex()
            .items_center()
            .justify_center()
            .cursor(gpui::CursorStyle::ResizeUpDown)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let expanded =
                        Section::ALL.map(|section| this.sidebar_expansion.is_expanded(section));
                    let content_height = (this.window_height
                        - this.top_chrome_height()
                        - SIDEBAR_VERTICAL_INSET * 2.0)
                        .max(0.0);
                    let heights =
                        section_heights(content_height, expanded, this.sidebar_section_weights);
                    let upper_index = section_index(upper);
                    let lower_index = section_index(lower);
                    this.sidebar_dragging = true;
                    this.sidebar_drag_offset = 0.0;
                    this.sidebar_split_drag = Some(SidebarSplitDrag {
                        upper: upper_index,
                        lower: lower_index,
                        start_y: f32::from(event.position.y),
                        start_upper_height: heights[upper_index],
                        pair_height: heights[upper_index] + heights[lower_index],
                        pair_weight: this.sidebar_section_weights[upper_index]
                            + this.sidebar_section_weights[lower_index],
                    });
                    cx.notify();
                }),
            )
            .child(div().mx(px(6.0)).h(px(1.0)).w_full().bg(border))
            .into_any_element()
    }

    /// 拖动期间的全窗口遮罩：覆盖侧栏与正文，接管两种方向的 move/up。
    pub(crate) fn render_sidebar_drag_overlay(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        use crate::frontend::gpui::model::sidebar_layout::{
            clamp_sidebar_width, resize_adjacent_weights, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
            SIDEBAR_SECTION_MIN_HEIGHT,
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
            .cursor(if self.sidebar_split_drag.is_some() {
                gpui::CursorStyle::ResizeUpDown
            } else {
                gpui::CursorStyle::ResizeLeftRight
            })
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button.is_none() {
                    return;
                }
                if let Some(drag) = this.sidebar_split_drag {
                    let requested =
                        drag.start_upper_height + f32::from(event.position.y) - drag.start_y;
                    let minimum = SIDEBAR_SECTION_MIN_HEIGHT.min(drag.pair_height / 2.0);
                    let upper_height = requested.clamp(minimum, drag.pair_height - minimum);
                    resize_adjacent_weights(
                        &mut this.sidebar_section_weights,
                        drag.upper,
                        drag.lower,
                        upper_height,
                        drag.pair_height,
                        drag.pair_weight,
                    );
                    cx.notify();
                } else {
                    let target = f32::from(event.position.x) - this.sidebar_drag_offset;
                    let next = clamp_sidebar_width(target, this.window_width, 320.0)
                        .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
                    if (next - this.sidebar_width).abs() > 0.5 {
                        this.sidebar_width = next;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    this.sidebar_dragging = false;
                    this.sidebar_split_drag = None;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    this.sidebar_dragging = false;
                    this.sidebar_split_drag = None;
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn render_section(
        &self,
        section: SidebarSectionModel,
        height: f32,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let kind = section.section;
        let expanded = section.expanded;
        let header_title = section_header_title(&section);
        let rows: Vec<AnyElement> = if expanded {
            section
                .rows
                .into_iter()
                .map(|row| self.render_sidebar_row(kind, row, theme, cx))
                .collect()
        } else {
            Vec::new()
        };
        let body_id = match kind {
            Section::Workspaces => "sidebar-scroll",
            Section::Agents => "sidebar-agent-scroll",
            Section::Commands => "sidebar-command-scroll",
            Section::HiddenCommands => "sidebar-hidden-command-scroll",
        };
        let header = div()
            .id(SharedString::from(format!("section-{kind:?}")))
            .debug_selector(move || format!("section-{kind:?}"))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(SECTION_HEIGHT))
            .flex_none()
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(icons::chevron_icon(expanded, theme.muted))
                    .child(SharedString::from(header_title)),
            )
            .child(div().flex().flex_row().items_center().gap(px(2.0)).when(
                kind == Section::Workspaces,
                |element| {
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
            ));

        let mut element = div()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .h(px(height))
            .min_h(px(SECTION_HEIGHT))
            .overflow_hidden()
            .child(header);
        if expanded {
            element = element.child(
                div()
                    .id(body_id)
                    .debug_selector(move || body_id.to_string())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .children(rows),
            );
        }
        element.into_any_element()
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
        // 行级选择器：名字行可能重名（Agents 行与 Workspace 行标题都是
        // `dev · local`），回归测试点整行时要用带身份的选择器。
        let row_selector = match (&target, section) {
            (RowTarget::Pane { pane_id, .. }, _) => {
                format!("sidebar-row-{section:?}-{pane_id}")
            }
            (RowTarget::Workspace(id), _) => format!("sidebar-row-workspace-{id}"),
        };
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
            .debug_selector({
                let selector = row_selector.clone();
                move || selector.clone()
            })
            .group(hover_group.clone())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_HEIGHT - ROW_SPACING))
            .flex_none()
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

fn section_index(section: Section) -> usize {
    match section {
        Section::Workspaces => 0,
        Section::Agents => 1,
        Section::Commands => 2,
        Section::HiddenCommands => 3,
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
