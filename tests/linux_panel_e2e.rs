//! 三 tab 面板 e2e（普通 GTK Window，不构造 AppWindow）。
//!
//! LINUX-PLAN C3.2/C3.3/C3.5：widget_name 契约、共享 Entry、Tab/Shift+Tab、
//! Esc、agent 状态、回车跳转。本机 xvfb/Mesa 在第三个窗口 present 时崩溃，
//! 因此整个 crate 只用一个 Window、一个面板生命周期。

#![cfg(feature = "gtk")]

mod support;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::process::Command;
use std::rc::Rc;
use std::time::Instant;

use gtk4::prelude::*;
use gtk4::{gdk, glib};
use support::linux_gtk::*;

use muxterm::core::attention::engine::PaneAttention;
use muxterm::core::attention::state::PaneStatus;
use muxterm::core::transport::ssh::probe::SshReach;
use muxterm::core::workspace::id::WorkspaceId;
use muxterm::platform::linux::panel_model::{PanelTab, SearchRow};
use muxterm::platform::linux::quickconnect::model::{
    QuickBadge, QuickConnectEntry, TargetConfig, TargetRuntime, TargetTransport,
};
use muxterm::platform::linux::quickconnect_panel::{show, PanelItem, PanelShowArgs};
use muxterm::platform::linux::workspace_sidebar::{ActivityIndicator, AgentSidebarItem};

fn attention(ws: &str, pane: u32, status: PaneStatus, line: &str) -> PaneAttention {
    PaneAttention {
        workspace_id: ws.into(),
        pane_id: pane,
        status,
        acknowledged: false,
        last_line: line.into(),
        seq: pane as u64,
        process_name: Some("cat".into()),
        process_is_agent: false,
        agent_name: None,
        shell_name: Some("zsh".into()),
        mute_until: None,
        last_regex_eval: Instant::now(),
    }
}

fn read_attention(ws: &str, pane: u32, status: PaneStatus, line: &str) -> PaneAttention {
    let mut attention = attention(ws, pane, status, line);
    attention.acknowledged = true;
    attention
}

fn agent(
    session: &str,
    path: &str,
    pane: u32,
    title: &str,
    detail: &str,
    indicator: ActivityIndicator,
) -> AgentSidebarItem {
    AgentSidebarItem {
        workspace_id: WorkspaceId::new("local", None, session, "tmux", path),
        pane_id: pane,
        title: title.into(),
        detail: detail.into(),
        indicator,
    }
}

fn target(name: &str) -> PanelItem {
    PanelItem::Target(
        QuickConnectEntry::new(
            TargetConfig::new(name, TargetRuntime::Tmux, TargetTransport::Local, "~/x"),
            vec![QuickBadge::Project],
        ),
        false,
    )
}

fn ssh_target(alias: &str) -> PanelItem {
    PanelItem::Target(
        QuickConnectEntry::new(
            TargetConfig::new(
                alias,
                TargetRuntime::Tmux,
                TargetTransport::Ssh { name: alias.into() },
                "~/x",
            ),
            vec![],
        ),
        false,
    )
}

fn entry_owns_window_focus(win: &gtk4::Window, entry: &gtk4::Entry) -> bool {
    gtk4::prelude::GtkWindowExt::focus(win).is_some_and(|focused| {
        focused == entry.clone().upcast::<gtk4::Widget>()
            || gtk4::prelude::WidgetExt::is_ancestor(&focused, entry)
    })
}

/// 每个 GTK 窗口生命周期单独进子进程。同一 test binary 里先后
/// present/destroy 两个 Window 会在 CI xvfb 上触发
/// `g_object_force_floating` + SIGSEGV（本文件原先也写了 Mesa 多窗崩溃）。
fn run_isolated(test_name: &'static str, body: impl FnOnce()) {
    if skip_no_display() {
        return;
    }
    if std::env::var_os("MUXTERM_PANEL_CHILD").is_none() {
        let executable = std::env::current_exe().expect("current test executable");
        let status = Command::new(executable)
            .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
            .env("MUXTERM_PANEL_CHILD", "1")
            .status()
            .unwrap_or_else(|error| panic!("spawn GTK panel child {test_name}: {error}"));
        assert!(
            status.success(),
            "GTK panel child {test_name} exited with {status}"
        );
        return;
    }
    body();
}

#[test]
fn three_tab_panel_full_flow() {
    run_isolated("three_tab_panel_full_flow", || {
        gtk4::test_synced(|| {
            gtk_test_framework_smoke();

            let win = gtk4::Window::builder()
                .title("panel-e2e")
                .default_width(800)
                .default_height(600)
                .build();
            win.present();
            gtk4::test_widget_wait_for_draw(&win);

            let jumps = Rc::new(RefCell::new(Vec::<(String, u32)>::new()));
            let j = jumps.clone();
            let long_detail = format!("/work/legion · main · {}", "x".repeat(180));
            show(
                &win,
                PanelShowArgs {
                    initial_tab: PanelTab::Attention,
                    workspaces: vec![
                        target("legion"),
                        target("muxterm"),
                        ssh_target("ryzen"),
                        ssh_target("dead"),
                    ],
                    agents: vec![
                        agent(
                            "legion",
                            "",
                            1,
                            "pi",
                            &long_detail,
                            ActivityIndicator::Running,
                        ),
                        agent(
                            "muxterm",
                            "",
                            2,
                            "codex",
                            "/work/muxterm · feature/panel",
                            ActivityIndicator::None,
                        ),
                    ],
                    attention: vec![
                        attention("legion@local", 1, PaneStatus::Working, "running"),
                        read_attention("muxterm@local", 2, PaneStatus::Done, "build ok"),
                        attention("plain@local", 3, PaneStatus::Blocked, "ask me"),
                    ],
                    on_connect: Box::new(|_| {}),
                    on_existing_connect: Box::new(|_| {}),
                    on_edit: Box::new(|_| {}),
                    on_new_project: Box::new(|| {}),
                    on_jump_pane: Box::new(move |ws, pane, _seq| j.borrow_mut().push((ws, pane))),
                    search: Box::new(|_, _| vec![]),
                    on_close: Box::new(|| {}),
                    ssh_reach: HashMap::from([
                        ("ryzen".into(), SshReach::Ok),
                        ("dead".into(), SshReach::Err),
                    ]),
                    existing: std::rc::Rc::new(std::cell::RefCell::new(
                        muxterm::platform::linux::quickconnect_panel::ExistingPanelState::default(),
                    )),
                    on_existing_nav: Box::new(|_| {}),
                },
            );
            pump_main_loop(80);

            // 1. 面板存在（widget_name 契约）
            let panel = find_by_name(&win, "muxterm-panel").expect("面板应存在");
            assert!(panel.is_visible());

            // 2. 初始 tab = Attention：只显示 running 与未读 done；已读 agent
            // 只常驻侧栏，不得继续占用快速面板。
            let list = find_by_name(&win, "muxterm-panel-list")
                .expect("列表应存在")
                .downcast::<gtk4::ListBox>()
                .expect("ListBox 类型");
            let labels = widget_label_texts(&list);
            assert!(
                labels.iter().any(|text| text == "pi")
                    && labels.iter().any(|text| text.contains("/work/legion")),
                "Working agent 应按 title + path/branch detail 展示: {labels:?}"
            );
            assert!(
                !labels.iter().any(|text| text == "codex"),
                "已读 agent 必须从 Attention 消失: {labels:?}"
            );
            assert!(
                labels.iter().any(|text| text.contains("ask me")),
                "非 agent 的 Blocked/Done attention 行仍须保留: {labels:?}"
            );

            // 3. 输入 query 过滤
            let entry = find_by_name(&win, "muxterm-panel-entry")
                .expect("共享 Entry 应存在")
                .downcast::<gtk4::Entry>()
                .expect("Entry 类型");
            entry.set_text("pi");
            pump_main_loop(40);
            let labels = widget_label_texts(&list);
            assert!(
                labels.iter().any(|text| text == "pi"),
                "title 过滤后应保留 pi: {labels:?}"
            );
            assert!(
                !labels.iter().any(|text| text.contains("ask me")),
                "过滤后应去掉其它命令: {labels:?}"
            );

            // 4. 清空 query：状态点正确；不再创建小终端或动作条。
            entry.set_text("");
            pump_main_loop(40);
            let pi_row = list.row_at_index(1).expect("pi agent row");
            let pi_dot =
                find_by_name(&pi_row, "muxterm-attention-status-dot").expect("pi status dot");
            assert!(
                pi_dot.has_css_class("running"),
                "Working agent 应显示黄色 running 状态 class"
            );
            let done_row = list.row_at_index(2).expect("unread done row");
            let done_dot = find_by_name(&done_row, "muxterm-attention-status-dot")
                .expect("unread done status dot");
            assert!(done_dot.has_css_class("done"));
            assert!(find_by_name(&win, "muxterm-attention-peek").is_none());
            assert!(find_by_name(&win, "muxterm-attention-jump").is_none());
            assert!(find_by_name(&win, "muxterm-attention-mute").is_none());

            // 5. 选择 + Enter 是 Attention 唯一动作。
            list.select_row(Some(&pi_row));
            entry.emit_activate();
            pump_main_loop(40);
            assert_eq!(
                jumps.borrow().as_slice(),
                &[("legion@local".to_string(), 1)]
            );

            // 6. 超长 agent detail 不得撑宽面板。
            gtk4::test_widget_wait_for_draw(&win);
            let attention_width = panel.width();
            assert!(
                attention_width > 0 && attention_width <= 640,
                "Attention 面板宽度必须有 640px 上限: {attention_width}"
            );

            // 7. Workspaces tab：注入的 SSH 灯（W15d），且切 tab 不改宽度。
            let ws_tab = find_by_name(&win, "muxterm-panel-tab-workspaces")
                .expect("Workspaces tab")
                .downcast::<gtk4::ToggleButton>()
                .expect("ToggleButton");
            let _: () = ws_tab.emit_by_name("clicked", &[]);
            pump_main_loop(80);
            gtk4::test_widget_wait_for_draw(&win);
            assert_eq!(
                panel.width(),
                attention_width,
                "切到 Workspaces 时面板不得横向抖动"
            );
            let ok_dot = find_by_name(&win, "muxterm-ssh-dot-ryzen")
                .expect("ryzen 行应有 muxterm-ssh-dot-ryzen");
            assert!(
                ok_dot.has_css_class("muxterm-ssh-dot-ok"),
                "ryzen 应为 ok class: {:?}",
                ok_dot.css_classes()
            );
            let err_dot = find_by_name(&win, "muxterm-ssh-dot-dead")
                .expect("dead 行应有 muxterm-ssh-dot-dead");
            assert!(
                err_dot.has_css_class("muxterm-ssh-dot-err"),
                "dead 应为 err class: {:?}",
                err_dot.css_classes()
            );

            // 8. 点 Search tab，占位行可见（从 Workspaces 起 Tab 会先到 Attention）
            let search_tab = find_by_name(&win, "muxterm-panel-tab-search")
                .expect("Search tab")
                .downcast::<gtk4::ToggleButton>()
                .expect("ToggleButton");
            let _: () = search_tab.emit_by_name("clicked", &[]);
            pump_main_loop(40);
            gtk4::test_widget_wait_for_draw(&win);
            assert_eq!(
                panel.width(),
                attention_width,
                "切到 Search 时面板不得横向抖动"
            );
            assert!(search_tab.is_active(), "应切到 Search");
            assert_eq!(entry.text().as_str(), "", "query 应跨 tab 保留");
            let status = find_by_name(&win, "muxterm-search-status").expect("搜索占位行");
            assert!(status.is_visible(), "Search tab 应显示占位行");

            // 9. Esc 关闭
            let entry_ctrl = window_key_controller(&entry).expect("Entry 应有 controller");
            simulate_key_press(&entry_ctrl, gdk::Key::Escape, gdk::ModifierType::empty());
            pump_main_loop(40);
            assert!(
                find_by_name(&win, "muxterm-panel").is_none(),
                "Esc 后面板应关闭"
            );

            win.close();
            win.destroy();
            pump_main_loop(40);
        });
    });
}

#[test]
fn keyboard_navigation_scrolls_selection_and_keeps_search_focus() {
    run_isolated(
        "keyboard_navigation_scrolls_selection_and_keeps_search_focus",
        || {
            gtk4::test_synced(|| {
                gtk_test_framework_smoke();
                let win = gtk4::Window::builder()
                    .title("panel-keyboard")
                    .default_width(800)
                    .default_height(600)
                    .build();
                win.present();
                gtk4::test_widget_wait_for_draw(&win);

                let connected = Rc::new(RefCell::new(Vec::<String>::new()));
                let connected_cb = connected.clone();
                show(
                    &win,
                    PanelShowArgs {
                        initial_tab: PanelTab::Workspaces,
                        workspaces: std::iter::once(target("legion"))
                            .chain(std::iter::once(target("muxterm")))
                            .chain((2..32).map(|i| target(&format!("workspace-{i:02}"))))
                            .collect(),
                        agents: vec![],
                        attention: vec![],
                        on_connect: Box::new(move |cfg| {
                            connected_cb.borrow_mut().push(cfg.name);
                        }),
                        on_existing_connect: Box::new(|_| {}),
                        on_edit: Box::new(|_| {}),
                        on_new_project: Box::new(|| {}),
                        on_jump_pane: Box::new(|_, _, _| {}),
                        search: Box::new(|_, _| vec![]),
                        on_close: Box::new(|| {}),
                        ssh_reach: HashMap::new(),
                        existing: Rc::new(RefCell::new(
                            muxterm::platform::linux::quickconnect_panel::ExistingPanelState::default(),
                        )),
                        on_existing_nav: Box::new(|_| {}),
                    },
                );
                pump_main_loop(80);

                let entry = find_by_name(&win, "muxterm-panel-entry")
                    .expect("共享搜索框应存在")
                    .downcast::<gtk4::Entry>()
                    .expect("Entry 类型");
                let list = find_by_name(&win, "muxterm-panel-list")
                    .expect("列表应存在")
                    .downcast::<gtk4::ListBox>()
                    .expect("ListBox 类型");
                assert!(
                    entry_owns_window_focus(&win, &entry),
                    "面板打开后焦点必须在搜索框"
                );

                let first = list.row_at_index(0).expect("第一条 workspace");
                assert!(first.height() > 0, "workspace 行必须完成分配");
                assert!(
                    first.height() <= 52,
                    "workspace 行应为紧凑的两行布局，实际高度 {}",
                    first.height()
                );

                let controller = window_key_controller(&entry).expect("Entry 应有键盘 controller");
                simulate_key_press(&controller, gdk::Key::Down, gdk::ModifierType::empty());
                pump_main_loop(10);
                assert_eq!(
                    list.selected_row().map(|row| row.index()),
                    Some(1),
                    "Down 应立即选择下一行"
                );
                assert!(
                    entry_owns_window_focus(&win, &entry),
                    "Down 后输入焦点必须仍在搜索框"
                );

                simulate_key_press(&controller, gdk::Key::Up, gdk::ModifierType::empty());
                pump_main_loop(10);
                assert_eq!(
                    list.selected_row().map(|row| row.index()),
                    Some(0),
                    "Up 应立即选择上一行"
                );
                assert!(
                    entry_owns_window_focus(&win, &entry),
                    "Up 后输入焦点必须仍在搜索框"
                );

                for _ in 0..20 {
                    simulate_key_press(&controller, gdk::Key::Down, gdk::ModifierType::empty());
                }
                pump_main_loop(20);
                let selected = list.selected_row().expect("连续 Down 后应有选中行");
                assert_eq!(selected.index(), 20, "连续 Down 应选中第 21 行");
                assert!(
                    entry_owns_window_focus(&win, &entry),
                    "滚动列表后输入焦点仍必须在搜索框"
                );
                let mut ancestor = list.clone().upcast::<gtk4::Widget>().parent();
                let scroller = loop {
                    let widget = ancestor.expect("列表必须位于 ScrolledWindow 中");
                    if let Ok(scroller) = widget.clone().downcast::<gtk4::ScrolledWindow>() {
                        break scroller;
                    }
                    ancestor = widget.parent();
                };
                let adjustment = scroller.vadjustment();
                let bounds = selected
                    .compute_bounds(&list)
                    .expect("选中行必须能换算到列表坐标");
                let row_top = f64::from(bounds.y());
                let row_bottom = f64::from(bounds.y() + bounds.height());
                let viewport_top = adjustment.value();
                let viewport_bottom = viewport_top + adjustment.page_size();
                assert!(
                    viewport_top > adjustment.lower(),
                    "选到溢出行后列表必须向下滚动：value={viewport_top}, page={} upper={}",
                    adjustment.page_size(),
                    adjustment.upper()
                );
                assert!(
                    row_top >= viewport_top - 1.0 && row_bottom <= viewport_bottom + 1.0,
                    "选中行必须完整位于视口：row={row_top}..{row_bottom}, viewport={viewport_top}..{viewport_bottom}"
                );

                for _ in 0..20 {
                    simulate_key_press(&controller, gdk::Key::Up, gdk::ModifierType::empty());
                }
                pump_main_loop(20);
                assert_eq!(
                    list.selected_row().map(|row| row.index()),
                    Some(0),
                    "连续 Up 应回到第一行"
                );
                assert!(
                    adjustment.value() <= adjustment.lower() + 1.0,
                    "选回第一行后列表必须滚回顶部：value={}, lower={}",
                    adjustment.value(),
                    adjustment.lower()
                );
                assert!(
                    entry_owns_window_focus(&win, &entry),
                    "向上滚回顶部后输入焦点仍必须在搜索框"
                );

                entry.set_text("legion");
                pump_main_loop(40);
                assert!(
                    entry_owns_window_focus(&win, &entry),
                    "过滤后输入焦点必须仍在搜索框"
                );
                assert_eq!(
                    list.selected_row().map(|row| row.index()),
                    Some(0),
                    "过滤结果应立即选中第一行"
                );
                assert!(
                    list.selected_row()
                        .is_some_and(|row| row.widget_name().contains("legion")),
                    "过滤后渲染行必须对应 legion，实际 {:?}",
                    list.selected_row().map(|row| row.widget_name())
                );
                // 真实键盘 Enter 由 GtkEntry 的 activate 信号表达。最后一个字符后
                // 不推进主循环，确保激活会先提交最新过滤条件，而不是打开旧选中项。
                entry.set_text("muxterm");
                entry.emit_activate();
                pump_main_loop(40);
                assert_eq!(connected.borrow().as_slice(), &["muxterm".to_string()]);
                assert!(
                    find_by_name(&win, "muxterm-panel").is_none(),
                    "Enter 激活可见行后应关闭面板"
                );

                win.close();
                win.destroy();
                pump_main_loop(40);
            });
        },
    );
}

#[test]
fn rapid_typing_and_attention_navigation_stay_lightweight() {
    run_isolated(
        "rapid_typing_and_attention_navigation_stay_lightweight",
        || {
            gtk4::test_synced(|| {
                gtk_test_framework_smoke();
                let win = gtk4::Window::builder()
                    .title("panel-coalescing")
                    .default_width(800)
                    .default_height(600)
                    .build();
                win.present();
                gtk4::test_widget_wait_for_draw(&win);

                let search_calls = Rc::new(Cell::new(0u32));
                let search_calls_cb = search_calls.clone();
                show(
                    &win,
                    PanelShowArgs {
                        initial_tab: PanelTab::Search,
                        workspaces: vec![target("muxterm")],
                        agents: vec![],
                        attention: vec![
                            attention("one", 1, PaneStatus::Blocked, "first"),
                            attention("two", 2, PaneStatus::Blocked, "second"),
                            attention("three", 3, PaneStatus::Done, "third"),
                        ],
                        on_connect: Box::new(|_| {}),
                        on_existing_connect: Box::new(|_| {}),
                        on_edit: Box::new(|_| {}),
                        on_new_project: Box::new(|| {}),
                        on_jump_pane: Box::new(|_, _, _| {}),
                        search: Box::new(move |query, _| {
                            search_calls_cb.set(search_calls_cb.get() + 1);
                            if query.is_empty() {
                                vec![]
                            } else {
                                vec![SearchRow {
                                    workspace_id: "muxterm".into(),
                                    tab_id: 1,
                                    pane_id: 1,
                                    seq: 1,
                                    line: query.to_string(),
                                }]
                            }
                        }),
                        on_close: Box::new(|| {}),
                        ssh_reach: HashMap::new(),
                        existing: Rc::new(RefCell::new(
                            muxterm::platform::linux::quickconnect_panel::ExistingPanelState::default(),
                        )),
                        on_existing_nav: Box::new(|_| {}),
                    },
                );
                pump_main_loop(80);

                let entry = find_by_name(&win, "muxterm-panel-entry")
                    .expect("共享搜索框应存在")
                    .downcast::<gtk4::Entry>()
                    .expect("Entry 类型");
                let baseline_search = search_calls.get();
                muxterm::platform::linux::quickconnect_panel::refresh_current();
                pump_main_loop(40);
                assert_eq!(
                    search_calls.get(),
                    baseline_search,
                    "已有连接探测刷新不得重建无关的 Search tab"
                );
                let main_context = glib::MainContext::default();
                entry.set_text("m");
                while main_context.iteration(false) {}
                assert_eq!(
                    search_calls.get(),
                    baseline_search,
                    "字符间的 ready source 调度不得立即执行昂贵搜索"
                );
                entry.set_text("mu");
                while main_context.iteration(false) {}
                assert_eq!(
                    search_calls.get(),
                    baseline_search,
                    "连续输入期间必须继续合并重建"
                );
                entry.set_text("mux");
                assert_eq!(
                    search_calls.get(),
                    baseline_search,
                    "连续输入不得在 changed 回调里同步执行搜索/重建"
                );
                pump_main_loop(40);
                assert_eq!(
                    search_calls.get(),
                    baseline_search + 1,
                    "一批连续输入只应执行一次最新查询"
                );
                assert!(
                    entry_owns_window_focus(&win, &entry),
                    "批量过滤后焦点必须仍在搜索框"
                );

                let attention_tab = find_by_name(&win, "muxterm-panel-tab-attention")
                    .expect("Attention tab")
                    .downcast::<gtk4::ToggleButton>()
                    .expect("ToggleButton");
                let _: () = attention_tab.emit_by_name("clicked", &[]);
                pump_main_loop(40);
                entry.set_text("");
                pump_main_loop(40);
                let list = find_by_name(&win, "muxterm-panel-list")
                    .expect("列表应存在")
                    .downcast::<gtk4::ListBox>()
                    .expect("ListBox 类型");
                let row_two = list.row_at_index(2).expect("第二条 attention");
                let row_three = list.row_at_index(3).expect("第三条 attention");
                list.select_row(Some(&row_two));
                list.select_row(Some(&row_three));
                list.select_row(Some(&row_two));
                assert_eq!(
                    list.selected_row().map(|row| row.index()),
                    Some(row_two.index()),
                    "快速切换后应立即停在最终选择行"
                );
                assert!(find_by_name(&win, "muxterm-attention-peek").is_none());
                assert!(
                    entry_owns_window_focus(&win, &entry),
                    "Attention 上下选择不应抢走搜索框焦点"
                );

                win.close();
                win.destroy();
                pump_main_loop(40);
            });
        },
    );
}

/// W20f / C9：已有的连接导航——点进去扁平列表，Back 回根。
#[test]
fn existing_connections_navigation() {
    run_isolated("existing_connections_navigation", || {
        gtk4::test_synced(|| {
            gtk_test_framework_smoke();
            let win = gtk4::Window::builder()
                .title("panel-existing")
                .default_width(800)
                .default_height(600)
                .build();
            win.present();
            gtk4::test_widget_wait_for_draw(&win);

            let existing = Rc::new(RefCell::new(
                muxterm::platform::linux::quickconnect_panel::ExistingPanelState::default(),
            ));
            let navs = Rc::new(RefCell::new(Vec::<String>::new()));
            let n = navs.clone();
            show(
                &win,
                PanelShowArgs {
                    initial_tab: PanelTab::Workspaces,
                    workspaces: vec![
                        PanelItem::Folder {
                            id: "existing-connections",
                            title: "已有的连接".into(),
                        },
                        target("muxterm"),
                        PanelItem::NewProject,
                    ],
                    agents: vec![],
                    attention: vec![],
                    on_connect: Box::new(|_| {}),
                    on_existing_connect: Box::new(|_| {}),
                    on_edit: Box::new(|_| {}),
                    on_new_project: Box::new(|| {}),
                    on_jump_pane: Box::new(|_, _, _| {}),
                    search: Box::new(|_, _| vec![]),
                    on_close: Box::new(|| {}),
                    ssh_reach: HashMap::new(),
                    existing: existing.clone(),
                    on_existing_nav: Box::new(move |nav| {
                        n.borrow_mut().push(format!("{nav:?}"));
                    }),
                },
            );
            pump_main_loop(80);

            // 根列表：第一行是已有的连接 Folder。
            let list = find_by_name(&win, "muxterm-panel-list")
                .expect("列表应存在")
                .downcast::<gtk4::ListBox>()
                .expect("ListBox 类型");
            let folder = find_by_name(&win, "muxterm-existing-connections")
                .expect("根列表第一项必须是 muxterm-existing-connections");
            assert!(folder.is_visible());
            let entry = find_by_name(&win, "muxterm-panel-entry")
                .expect("共享搜索框应存在")
                .downcast::<gtk4::Entry>()
                .expect("Entry 类型");

            // Enter Folder → 扁平列表：Back，禁止本地/SSH 目录。
            assert_eq!(list.selected_row().map(|row| row.index()), Some(0));
            entry.emit_activate();
            pump_main_loop(60);
            assert!(
                find_by_name(&win, "muxterm-existing-local").is_none(),
                "已有的连接禁止再出现 muxterm-existing-local 目录"
            );
            assert!(
                find_by_name(&win, "muxterm-existing-ssh").is_none(),
                "已有的连接禁止再出现 muxterm-existing-ssh 目录"
            );
            assert!(
                find_by_name(&win, "muxterm-existing-back").is_some(),
                "扁平列表应有 muxterm-existing-back"
            );

            // Back 回根：New Project 还在。
            let list = find_by_name(&win, "muxterm-panel-list")
                .expect("列表应存在")
                .downcast::<gtk4::ListBox>()
                .expect("ListBox 类型");
            assert_eq!(list.selected_row().map(|row| row.index()), Some(0));
            entry.emit_activate();
            pump_main_loop(60);
            assert!(
                find_by_name(&win, "__new_project__").is_some(),
                "回到根后 New Project 必须还在"
            );
            assert!(
                find_by_name(&win, "muxterm-existing-connections").is_some(),
                "回到根后已有的连接 Folder 必须还在"
            );
            assert!(
                !navs.borrow().is_empty(),
                "导航回调应被触发: {:?}",
                navs.borrow()
            );
        });
    });
}
