//! 左侧栏布局契约：断言真实段头的位置和真实鼠标事件，不用模型结果作预期值。
use super::keys::keys;
use super::{mount, store_with_two_workspaces, workspace};
use gpui::{
    point, px, size, Bounds, Modifiers, MouseButton, Pixels, ScrollDelta, ScrollWheelEvent,
    TestAppContext, TouchPhase, VisualTestContext,
};

const W: &str = "section-Workspaces";
const A: &str = "section-Agents";
const C: &str = "section-Commands";
const H: &str = "section-HiddenCommands";
const HEADER: f32 = 26.;

fn bounds(cx: &mut VisualTestContext, id: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(id).unwrap_or_else(|| panic!("缺少 {id}"))
}

fn top(cx: &mut VisualTestContext, id: &'static str) -> f32 {
    bounds(cx, id).origin.y.into()
}

fn near(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 1.1,
        "{what}: 实际 {actual}，应为 {expected}"
    );
}

fn click(cx: &mut VisualTestContext, header: &'static str) {
    let position = bounds(cx, header).center();
    cx.simulate_click(position, Modifiers::none());
    cx.run_until_parked();
}

/// 折叠段头连续排到侧栏下沿，没有列表、保留空位或被裁掉的段头。
fn dock(cx: &mut VisualTestContext, headers: &[&'static str]) {
    let sidebar = bounds(cx, "sidebar");
    let mut y = f32::from(sidebar.bottom()) - 4.0 - HEADER * headers.len() as f32;
    for &header in headers {
        let rect = bounds(cx, header);
        near(rect.origin.y.into(), y, header);
        near(rect.size.height.into(), HEADER, "折叠段只占一个段头");
        y += HEADER;
    }
}

fn open_heights(cx: &mut VisualTestContext) -> [f32; 3] {
    let [w, a, c, h] = [W, A, C, H].map(|id| top(cx, id));
    [a - w, c - a, h - c]
}

/// 对照 SidebarSectionSplitLayoutTests.testDefaultExpandedLayoutFillsTheSidebar；
/// GPUI 的默认份额以用户确认为准：W 1/2、A 1/4、C 1/4，和条目数无关。
#[gpui::test]
fn left_sidebar_defaults_to_half_quarter_quarter_across_window_sizes(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let expanded: Vec<_> = cx.update(|_, cx| {
        view.read(cx)
            .sidebar_model()
            .into_iter()
            .map(|s| s.expanded)
            .collect()
    });
    assert_eq!(expanded, [true, true, true, false]);
    for height in [800., 542., 975.] {
        cx.simulate_resize(size(px(1200.), px(height)));
        cx.run_until_parked();
        let sidebar = bounds(cx, "sidebar");
        near(sidebar.origin.x.into(), 0., "侧栏留在左边");
        near(sidebar.size.width.into(), 240., "默认宽度");
        near(
            top(cx, W),
            f32::from(sidebar.origin.y) + 4.0,
            "第一个展开段贴顶",
        );
        let available = f32::from(sidebar.size.height) - 8.0 - HEADER;
        for (idx, measured) in open_heights(cx).into_iter().enumerate() {
            near(measured, available * [0.5, 0.25, 0.25][idx], [W, A, C][idx]);
        }
        dock(cx, &[H]);
    }
}

/// 对照 SidebarSectionSplitLayoutTests.testAllCollapsedKeepsHeadersAndPushesSlackToTheLastPane；
/// 新契约不同：空白在段头上方，全部折叠也没有任何一段吃掉空白。
#[gpui::test]
fn any_folded_section_joins_the_bottom_and_can_reopen_in_place(cx: &mut TestAppContext) {
    let (_view, cx) = mount(store_with_two_workspaces(), cx);
    click(cx, A);
    dock(cx, &[A, H]);
    assert!(top(cx, W) < top(cx, C) && top(cx, C) < top(cx, A));
    click(cx, W);
    dock(cx, &[W, A, H]);
    near(
        top(cx, C),
        f32::from(bounds(cx, "sidebar").origin.y) + 4.0,
        "剩余的 Commands 应接管顶部可用空间",
    );
    click(cx, C);
    dock(cx, &[W, A, C, H]);
    click(cx, H);
    near(
        top(cx, H),
        f32::from(bounds(cx, "sidebar").origin.y) + 4.0,
        "底部的 Hidden Commands 同样可展开",
    );
    dock(cx, &[W, A, C]);
    click(cx, H);
    for header in [C, W, A] {
        click(cx, header);
    }
    let [w, a, c] = open_heights(cx);
    near(w, 2. * a, "展开后恢复原顺序与份额");
    near(c, a, "Commands 恢复原份额");
    dock(cx, &[H]);
}

fn drag_height(cx: &mut VisualTestContext, divider: &'static str, dy: f32, target_x: f32) {
    let grab = bounds(cx, divider).center();
    cx.simulate_mouse_down(grab, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let to = point(px(target_x), grab.y + px(dy));
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

/// 对照 SidebarSectionSplitLayoutTests.testFramesCoverTheSplitViewInArrangedOrder；
/// 真拖动两条横线：上下限、相邻借位、折叠/改窗高/隐藏侧栏不能重置用户的分配。
#[gpui::test]
fn height_drags_only_trade_space_with_the_other_side_of_the_divider(cx: &mut TestAppContext) {
    let (_view, cx) = mount(store_with_two_workspaces(), cx);
    let before = open_heights(cx);
    drag_height(cx, "sidebar-split-Agents-Commands", 46., 700.);
    let [w, a, c] = open_heights(cx);
    near(w, before[0], W);
    near(a, before[1] + 46., "Agents 可以更多");
    near(c, before[2] - 46., "Commands 同量变小");
    near(
        bounds(cx, "sidebar").size.width.into(),
        240.,
        "上下拖不改宽度",
    );
    click(cx, A);
    click(cx, A);
    cx.simulate_keystrokes(&keys("cmd-b cmd-b"));
    for (actual, wanted) in open_heights(cx).into_iter().zip([w, a, c]) {
        near(actual, wanted, "重新显示后保留拖动结果");
    }
    cx.simulate_resize(size(px(1200.), px(1026.)));
    cx.run_until_parked();
    // 可分配高度从实测侧栏扣 inset 与折叠段头得到：非 macOS 顶部还有菜单条，
    // 不能写死 1026 - 8 - 26。
    let available = f32::from(bounds(cx, "sidebar").size.height) - 8.0 - HEADER;
    for (actual, fraction) in open_heights(cx).into_iter().zip([w, a, c]) {
        near(
            actual,
            available * fraction / (w + a + c),
            "随窗口按比例缩放",
        );
    }
    let prev = open_heights(cx);
    drag_height(cx, "sidebar-split-Workspaces-Agents", -460., 100.);
    let after = open_heights(cx);
    near(after[0], 80., "分区不能被拖没");
    near(after[1], prev[0] + prev[1] - 80., "多余高度归相邻段");
    near(after[2], prev[2], "拖上方横线不改 Commands");
    dock(cx, &[H]);
}

/// 对照 WorkspaceSidebarE2ETests：往左拖进入侧栏内部，而不是仅测往右进入终端。
#[gpui::test]
fn width_drag_tracks_and_releases_inside_the_sidebar_without_changing_heights(
    cx: &mut TestAppContext,
) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let before = open_heights(cx);
    let grab = bounds(cx, "sidebar-divider").center();
    cx.simulate_mouse_down(grab, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let to = point(px(125.), grab.y);
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    near(
        bounds(cx, "sidebar").size.width.into(),
        200.,
        "向左拖至下限",
    );
    assert!(
        !cx.update(|_, cx| view.read(cx).sidebar_dragging_for_tests()),
        "在侧栏内松手也要结束拖动"
    );
    assert_eq!(open_heights(cx), before);
}

/// 对照 WorkspaceSidebarE2ETests：长列表只滚段内正文，其他段头和折叠区不动。
#[gpui::test]
fn scrolling_workspace_rows_does_not_scroll_other_sections(cx: &mut TestAppContext) {
    let mut store = store_with_two_workspaces();
    for i in 0..40 {
        let name = format!("project-{i}");
        store.replace_topology(workspace(&name, &name, false), vec![], vec![]);
    }
    let (_view, cx) = mount(store, cx);
    let headers = [W, A, C, H].map(|id| top(cx, id));
    let row = "sidebar-name-dev";
    let before = bounds(cx, row);
    cx.simulate_mouse_move(before.center(), None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: before.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
    assert!(
        top(cx, row) < f32::from(before.origin.y) - 5.,
        "Workspaces 自身必须确实滚动"
    );
    assert_eq!(
        [W, A, C, H].map(|id| top(cx, id)),
        headers,
        "滚动不能挪走分隔条或段头"
    );
    dock(cx, &[H]);
}
