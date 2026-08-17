//! 三 tab 面板 e2e（普通 GTK Window，不构造 AppWindow）。
//!
//! LINUX-PLAN C3.2/C3.3/C3.5：widget_name 契约、共享 Entry、Tab/Shift+Tab、
//! Esc、peek、一行答复、静音。本机 xvfb/Mesa 在第三个窗口 present 时崩溃，
//! 因此整个 crate 只用一个 Window、一个面板生命周期。

#![cfg(feature = "gtk")]

mod support;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::gdk;
use gtk4::prelude::*;
use support::linux_gtk::*;
use vte4::prelude::*;

use muxterm::core::attention::engine::PaneAttention;
use muxterm::core::attention::state::PaneStatus;
use muxterm::core::config::Theme;
use muxterm::core::transport::ssh::probe::SshReach;
use muxterm::platform::linux::panel_model::PanelTab;
use muxterm::platform::linux::quickconnect::font::FontSettings;
use muxterm::platform::linux::quickconnect::model::{
    QuickBadge, QuickConnectEntry, TargetConfig, TargetRuntime, TargetTransport,
};
use muxterm::platform::linux::quickconnect_panel::{
    show, test_emit_peek_input, PanelItem, PanelShowArgs,
};

fn attention(ws: &str, pane: u32, status: PaneStatus, line: &str) -> PaneAttention {
    PaneAttention {
        workspace_id: ws.into(),
        pane_id: pane,
        status,
        last_line: line.into(),
        seq: pane as u64,
        process_name: Some("cat".into()),
        mute_until: None,
        last_regex_eval: Instant::now(),
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

#[test]
fn three_tab_panel_full_flow() {
    if skip_no_display() {
        return;
    }
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
        let inputs = Rc::new(RefCell::new(Vec::<(String, u32, Vec<u8>)>::new()));
        let mutes = Rc::new(RefCell::new(Vec::<(String, u32, Duration)>::new()));
        let j = jumps.clone();
        let i = inputs.clone();
        let m = mutes.clone();
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
                attention: vec![
                    attention("legion", 1, PaneStatus::Blocked, "ask me"),
                    attention("muxterm", 2, PaneStatus::Done, "build ok"),
                ],
                theme: Theme::load("light").unwrap_or_else(|_| Theme {
                    name: "test".into(),
                    background: muxterm::core::config::Rgb(0x1e, 0x1e, 0x2e),
                    foreground: muxterm::core::config::Rgb(0xcd, 0xd6, 0xf4),
                    cursor: muxterm::core::config::Rgb(0xf5, 0xe0, 0xdc),
                    colors: [muxterm::core::config::Rgb(0, 0, 0); 16],
                }),
                font: FontSettings::default(),
                on_connect: Box::new(|_| {}),
                on_edit: Box::new(|_| {}),
                on_new_project: Box::new(|| {}),
                on_jump_pane: Box::new(move |ws, pane, _seq| j.borrow_mut().push((ws, pane))),
                on_send_input: Box::new(move |ws, pane, data| {
                    i.borrow_mut().push((ws, pane, data.to_vec()))
                }),
                on_mute: Box::new(move |ws, pane, d| m.borrow_mut().push((ws, pane, d))),
                peek_bytes: Box::new(|ws, pane| {
                    (80, 24, format!("\x1b[1;1Hpeek-{ws}-{pane}").into_bytes())
                }),
                search: Box::new(|_| vec![]),
                on_close: Box::new(|| {}),
                ssh_reach: HashMap::from([
                    ("ryzen".into(), SshReach::Ok),
                    ("dead".into(), SshReach::Err),
                ]),
            },
        );
        pump_main_loop(80);

        // 1. 面板存在（widget_name 契约）
        let panel = find_by_name(&win, "muxterm-panel").expect("面板应存在");
        assert!(panel.is_visible());

        // 2. 初始 tab = Attention：列表含工作区名与 last_line
        let list = find_by_name(&win, "muxterm-panel-list")
            .expect("列表应存在")
            .downcast::<gtk4::ListBox>()
            .expect("ListBox 类型");
        let labels = widget_label_texts(&list);
        assert!(
            labels
                .iter()
                .any(|t| t.contains("legion") && t.contains("ask me")),
            "Tab2 应显示工作区+进程+last_line: {labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|t| t.contains("muxterm") && t.contains("build ok")),
            "Tab2 应显示第二条: {labels:?}"
        );

        // 3. 输入 query 过滤
        let entry = find_by_name(&win, "muxterm-panel-entry")
            .expect("共享 Entry 应存在")
            .downcast::<gtk4::Entry>()
            .expect("Entry 类型");
        entry.set_text("legion");
        pump_main_loop(40);
        let labels = widget_label_texts(&list);
        assert!(
            labels.iter().any(|t| t.contains("legion")),
            "过滤后应保留 legion: {labels:?}"
        );
        assert!(
            !labels.iter().any(|t| t.contains("build ok")),
            "过滤后应去掉 muxterm: {labels:?}"
        );

        // 4. 清空 query，选中 legion 行 → 小 VTE 播种 + 按钮可用
        entry.set_text("");
        pump_main_loop(40);
        let row = list.row_at_index(1).expect("注意力行");
        list.select_row(Some(&row));
        // select_row 不触发信号；rebuild 会按当前选中行刷新小 VTE。
        entry.set_text("x");
        entry.set_text("");
        pump_main_loop(40);

        let peek_sw = find_by_name(&win, "muxterm-attention-peek")
            .expect("小 VTE 容器应存在")
            .downcast::<gtk4::ScrolledWindow>()
            .expect("ScrolledWindow 类型");
        let peek_term = peek_sw
            .child()
            .expect("小 VTE 应有子控件")
            .downcast::<vte4::Terminal>()
            .expect("子控件应为 VTE Terminal");
        let text = peek_term
            .text_format(vte4::Format::Text)
            .map(|s| s.to_string())
            .unwrap_or_default();
        assert!(
            text.contains("peek-legion-1"),
            "小 VTE 应显示 replica 播种内容: {text:?}"
        );

        // 5. 跳转按钮 → on_jump_pane
        let jump_btn = find_by_name(&win, "muxterm-attention-jump")
            .expect("跳转按钮应存在")
            .downcast::<gtk4::Button>()
            .expect("Button 类型");
        assert!(jump_btn.is_sensitive(), "选中后跳转应可用");
        let _: () = jump_btn.emit_by_name("clicked", &[]);
        pump_main_loop(40);
        let got = jumps.borrow();
        assert_eq!(got.len(), 1, "应恰好跳转一次");
        assert_eq!(got[0], ("legion".to_string(), 1));
        drop(got);

        // 5b. 小 VTE 快速回复 → on_send_input（W15e）
        test_emit_peek_input(b"REPLY_PANEL");
        pump_main_loop(40);
        let got = inputs.borrow();
        assert_eq!(got.len(), 1, "peek 输入应走 on_send_input 一次: {got:?}");
        assert_eq!(got[0].0, "legion");
        assert_eq!(got[0].1, 1);
        assert_eq!(got[0].2, b"REPLY_PANEL".to_vec());
        drop(got);

        // 6. 放大按钮：小 VTE 高度 120 → 360
        let zoom_btn = find_by_name(&win, "muxterm-attention-zoom")
            .expect("放大按钮应存在")
            .downcast::<gtk4::Button>()
            .expect("Button 类型");
        assert!(zoom_btn.is_sensitive(), "选中后放大应可用");
        assert_eq!(peek_sw.height_request(), 120, "初始小 VTE 高 120");
        let _: () = zoom_btn.emit_by_name("clicked", &[]);
        pump_main_loop(40);
        assert_eq!(peek_sw.height_request(), 360, "放大后小 VTE 高 360");

        // 7. 禁止提醒下拉：mute-10m → on_mute(ws, pane, 600s)
        let mute_btn = find_by_name(&win, "muxterm-attention-mute")
            .expect("静音下拉应存在")
            .downcast::<gtk4::MenuButton>()
            .expect("MenuButton 类型");
        assert!(mute_btn.is_sensitive(), "选中后静音应可用");
        let mute_10m = find_by_name(&win, "muxterm-attention-mute-10m")
            .expect("10m 菜单项应存在")
            .downcast::<gtk4::Button>()
            .expect("Button 类型");
        let _: () = mute_10m.emit_by_name("clicked", &[]);
        pump_main_loop(40);
        let got = mutes.borrow();
        assert_eq!(got.len(), 1, "应恰好静音一次");
        assert_eq!(got[0].0, "legion");
        assert_eq!(got[0].1, 1);
        assert_eq!(got[0].2, Duration::from_secs(600), "10m 应回调 600s");
        drop(got);

        // 7b. Workspaces tab：注入的 SSH 灯（W15d）
        let ws_tab = find_by_name(&win, "muxterm-panel-tab-workspaces")
            .expect("Workspaces tab")
            .downcast::<gtk4::ToggleButton>()
            .expect("ToggleButton");
        let _: () = ws_tab.emit_by_name("clicked", &[]);
        pump_main_loop(80);
        let ok_dot = find_by_name(&win, "muxterm-ssh-dot-ryzen")
            .expect("ryzen 行应有 muxterm-ssh-dot-ryzen");
        assert!(
            ok_dot.has_css_class("muxterm-ssh-dot-ok"),
            "ryzen 应为 ok class: {:?}",
            ok_dot.css_classes()
        );
        let err_dot =
            find_by_name(&win, "muxterm-ssh-dot-dead").expect("dead 行应有 muxterm-ssh-dot-dead");
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
}
