//! 三 tab 面板 e2e（普通 GTK Window，不构造 AppWindow）。
//!
//! LINUX-PLAN C3.2/C3.3/C3.5：widget_name 契约、共享 Entry、Tab/Shift+Tab、
//! Esc、peek、一行答复、静音。本机 xvfb/Mesa 在第三个窗口 present 时崩溃，
//! 因此整个 crate 只用一个 Window、一个面板生命周期。

#![cfg(feature = "gtk")]

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gtk4::gdk;
use gtk4::prelude::*;
use support::linux_gtk::*;

use muxterm::core::attention::engine::PaneAttention;
use muxterm::core::attention::state::PaneStatus;
use muxterm::platform::linux::panel_model::PanelTab;
use muxterm::platform::linux::quickconnect::model::{
    QuickBadge, QuickConnectEntry, TargetConfig, TargetRuntime, TargetTransport,
};
use muxterm::platform::linux::quickconnect_panel::{show, PanelItem, PanelShowArgs};

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
        let replies = Rc::new(RefCell::new(Vec::<(String, u32, String)>::new()));
        let mutes = Rc::new(RefCell::new(Vec::<(String, u32)>::new()));
        let j = jumps.clone();
        let r = replies.clone();
        let m = mutes.clone();
        show(
            &win,
            PanelShowArgs {
                initial_tab: PanelTab::Attention,
                workspaces: vec![target("legion"), target("muxterm")],
                attention: vec![
                    attention("legion", 1, PaneStatus::Blocked, "ask me"),
                    attention("muxterm", 2, PaneStatus::Done, "build ok"),
                ],
                on_connect: Box::new(|_| {}),
                on_edit: Box::new(|_| {}),
                on_new_project: Box::new(|| {}),
                on_jump_pane: Box::new(move |ws, pane| j.borrow_mut().push((ws, pane))),
                on_reply: Box::new(move |ws, pane, text| r.borrow_mut().push((ws, pane, text))),
                on_mute: Box::new(move |ws, pane| m.borrow_mut().push((ws, pane))),
                peek_text: Box::new(|ws, pane| format!("peek-{ws}-{pane}\nline2")),
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

        // 4. 清空 query，选中 legion 行 → peek + 答复目标 + 静音可用
        entry.set_text("");
        pump_main_loop(40);
        let row = list.row_at_index(1).expect("注意力行");
        list.select_row(Some(&row));
        // select_row 不触发信号；rebuild 会按当前选中行刷新 peek。
        entry.set_text("x");
        entry.set_text("");
        pump_main_loop(40);

        let peek = find_by_name(&win, "muxterm-peek-view")
            .expect("peek 视图应存在")
            .downcast::<gtk4::TextView>()
            .expect("TextView 类型");
        let buf = peek.buffer();
        let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
        assert!(
            text.contains("peek-legion-1"),
            "peek 应显示副本尾部: {text:?}"
        );

        let reply_entry = find_by_name(&win, "muxterm-reply-entry")
            .expect("答复输入框应存在")
            .downcast::<gtk4::Entry>()
            .expect("Entry 类型");
        assert!(reply_entry.is_sensitive(), "选中后答复应可用");
        let target = find_by_name(&win, "muxterm-reply-target")
            .expect("目标标签应存在")
            .downcast::<gtk4::Label>()
            .expect("Label 类型");
        assert!(
            target.text().contains("legion"),
            "目标标签应含工作区: {}",
            target.text()
        );

        // 5. 一行答复：Enter 发送且无换行
        reply_entry.set_text("y\n");
        let ctrl = window_key_controller(&reply_entry).expect("答复 Entry 应有 controller");
        simulate_key_press(&ctrl, gdk::Key::Return, gdk::ModifierType::empty());
        pump_main_loop(40);
        let got = replies.borrow();
        assert_eq!(got.len(), 1, "应恰好发送一条答复");
        assert_eq!(got[0].0, "legion");
        assert_eq!(got[0].1, 1);
        assert!(
            !got[0].2.contains('\n'),
            "答复文本不应含换行: {:?}",
            got[0].2
        );
        assert_eq!(reply_entry.text().as_str(), "", "发送后输入框应清空");
        drop(got);

        // 6. 静音 1h：按钮触发 on_mute
        let mute_btn = find_by_name(&win, "muxterm-mute-1h")
            .expect("静音按钮应存在")
            .downcast::<gtk4::Button>()
            .expect("Button 类型");
        assert!(mute_btn.is_sensitive(), "选中后静音应可用");
        let _: () = mute_btn.emit_by_name("clicked", &[]);
        pump_main_loop(40);
        let got = mutes.borrow();
        assert_eq!(got.len(), 1, "应恰好静音一次");
        assert_eq!(got[0], ("legion".to_string(), 1));
        drop(got);

        // 7. Tab 键切到 Search，entry 文本仍在
        let entry_ctrl = window_key_controller(&entry).expect("Entry 应有 controller");
        simulate_key_press(&entry_ctrl, gdk::Key::Tab, gdk::ModifierType::empty());
        pump_main_loop(40);
        let search_tab = find_by_name(&win, "muxterm-panel-tab-search").expect("Search tab");
        let search_btn = search_tab.downcast::<gtk4::ToggleButton>().unwrap();
        assert!(search_btn.is_active(), "Tab 键应切到 Search");
        assert_eq!(entry.text().as_str(), "", "query 应跨 tab 保留");
        let status = find_by_name(&win, "muxterm-search-status").expect("搜索占位行");
        assert!(status.is_visible(), "Search tab 应显示占位行");

        // 8. Esc 关闭
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
