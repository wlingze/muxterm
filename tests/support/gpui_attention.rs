//! Agent 通知已读：用真实窗口事件验铃铛、面板、Agents 行和 tab 是否一致。
use super::{
    backend_status, mount, workspace, ClientPane, ClientTab, GpuiApp, Indicator, RowTarget,
    Section, ViewStore,
};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
use muxterm::frontend::gpui::model::attention::{
    AttentionSnapshot, PaneAttention, PaneAttentionStatus as Status, WorkspaceAttention,
};
use muxterm::frontend::utils::corebridge::ClientLayout;

const DEV: &str = "local//dev/tmux/~";
const OPS: &str = "ssh/lab/ops/tmux/~";

fn agent(pane_id: u32, status: Status) -> PaneAttention {
    PaneAttention {
        pane_id,
        status,
        process_is_agent: true,
        agent_name: Some("pi".into()),
        ..Default::default()
    }
}

fn fixture(
    cx: &mut TestAppContext,
    workspaces: Vec<WorkspaceAttention>,
) -> (Entity<GpuiApp>, &mut VisualTestContext) {
    let mut store = ViewStore::default();
    for (id, name) in [(DEV, "dev"), (OPS, "ops")] {
        let tab = |id| ClientTab {
            id,
            name: format!("tab-{id}"),
            is_active: id == 1,
        };
        let pane = |id| ClientPane {
            id,
            cols: 80,
            rows: 24,
            is_active: id != 3,
            title: "pi".into(),
        };
        store.replace_topology(
            workspace(id, name, id == DEV),
            vec![tab(1), tab(2)],
            vec![(1, vec![pane(1)]), (2, vec![pane(2), pane(3)])],
        );
        store.replace_layouts(id, vec![(1, ClientLayout::Leaf { pane_id: 1 })]);
        store.ensure_workspace(id).backend_status = Some(backend_status::CONNECTED);
    }
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_attention(Some(AttentionSnapshot {
                workspaces,
                ..Default::default()
            }));
            cx.notify();
        })
    });
    (view, cx)
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let point = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("没有渲染 {selector}"))
        .center();
    cx.simulate_click(point, Modifiers::none());
}

/// 对照 AttentionSemanticsE2ETests.testBlockedLeavesAttentionAfterViewAndRegexLightsAgain。
/// 点击远端通知，不能顺便读掉本机同号 pane、目的工作区默认 pane 或目的 tab 的另一格。
#[gpui::test]
fn clicking_an_attention_row_reads_only_the_target_agent(cx: &mut TestAppContext) {
    let (view, cx) = fixture(
        cx,
        vec![
            WorkspaceAttention::new(DEV, vec![agent(3, Status::Blocked)]),
            WorkspaceAttention::new(OPS, (1..=3).map(|id| agent(id, Status::Blocked)).collect()),
        ],
    );
    click(cx, "status-attention");
    let i = cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.status_attention().count, 4, "只打开铃铛不能标成已读");
        app.panel_rows().iter().position(|row| matches!(row, PanelRow::Attention { workspace_id, pane_id: 3, .. } if workspace_id == OPS)).expect("目标未读提醒")
    });
    click(
        cx,
        ["panel-row-0", "panel-row-1", "panel-row-2", "panel-row-3"][i],
    );
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(
            (app.visible_workspace(), app.focused_pane()),
            (Some(OPS), Some(3))
        );
        assert_eq!(app.status_attention().count, 3, "看过一条，铃铛只应少一条");
    });
    click(cx, "status-attention");
    cx.update(|_, cx| {
        let app = view.read(cx);
        let rows = app.panel_rows();
        let has = |ws, id| rows.iter().any(|row| matches!(row, PanelRow::Attention { workspace_id, pane_id, .. } if workspace_id == ws && *pane_id == id));
        assert!(!has(OPS, 3) && has(DEV, 3) && has(OPS, 1) && has(OPS, 2), "已读作用域错误：{rows:?}");
        let section = app.sidebar_model().into_iter().find(|s| s.section == Section::Agents).unwrap();
        let target = section.rows.iter().find(|row| matches!(&row.target, RowTarget::Pane { workspace_id, pane_id: 3, .. } if workspace_id == OPS)).expect("已读的 Agent 仍应常驻");
        assert!(
            target.detail.starts_with("blocked"),
            "查看不改变 Agent 实际状态"
        );
        assert_eq!(
            target.indicator,
            Indicator::Blocked,
            "已读只退出提醒列表；Agent 仍真的被阻塞"
        );
    });
}

/// 对照 AttentionBadgeE2ETests.testBelPaintsBadge：已聚焦的 Agent 不需要先切走才能查看。
#[gpui::test]
fn clicking_the_focused_agent_in_the_sidebar_clears_its_badge_and_tab_mark(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(
        cx,
        vec![WorkspaceAttention::new(DEV, vec![agent(1, Status::Done)])],
    );
    click(cx, "sidebar-row-Agents-1");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(
            !app.status_attention().is_active(),
            "点当前 Agent 也必须清除未读"
        );
        assert_eq!(app.tab_items()[0].activity, None);
        assert_eq!(app.sidebar_model()[1].rows.len(), 1);
    });
}

/// 对照 AttentionNavE2ETests.testEnterJumpsWithoutPeekAndCmdEnterOpensOverlay：
/// 直接去现场查看与通过提醒跳转等价；聚焦 tab 不等于看过所有分屏。
#[gpui::test]
fn clicking_a_pane_or_tab_reads_only_its_focused_pane(cx: &mut TestAppContext) {
    let (view, cx) = fixture(
        cx,
        vec![WorkspaceAttention::new(
            DEV,
            (1..=3).map(|id| agent(id, Status::Done)).collect(),
        )],
    );
    click(cx, "pane-text-1");
    assert_eq!(
        cx.update(|_, cx| view.read(cx).status_attention().count),
        2,
        "直接点 pane 就是查看"
    );
    click(cx, "tab-2");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.status_attention().count, 1, "只清新 tab 的活动 pane");
        assert_eq!(
            app.tab_items()[0].activity,
            None,
            "看过的 done 不再点亮 tab"
        );
        assert_eq!(
            app.tab_items()[1].activity,
            Some(Indicator::Done),
            "同一 tab 里没聚焦的分屏仍未读"
        );
    });
}
