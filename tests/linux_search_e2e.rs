//! Search tab e2e（普通 GTK Window，无 AppWindow）：replica 命中 → hit widget → 激活跳转。
//!
//! LINUX-PLAN E5：Search tab 接 ReplicaStore.search，不再只是占位编译。

#![cfg(feature = "gtk")]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use support::linux_gtk::*;

use muxterm::core::config::Theme;
use muxterm::core::model::backend::mock::MockRuntime;
use muxterm::core::types::PaneId;
use muxterm::core::workspace::id::WorkspaceId;
use muxterm::core::workspace::workspace::Workspace;
use muxterm::platform::linux::panel_model::{PanelTab, SearchRow};
use muxterm::platform::linux::quickconnect::font::FontSettings;
use muxterm::platform::linux::quickconnect_panel::{show, PanelShowArgs};

#[test]
fn search_tab_finds_replica_hits_and_jumps() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();

        let win = gtk4::Window::builder()
            .title("search-e2e")
            .default_width(800)
            .default_height(600)
            .build();
        win.present();
        gtk4::test_widget_wait_for_draw(&win);

        // 注入含 TOKEN_BODY 的工作区 PaneBuf（core Workspace 路径）。
        let ws = Rc::new(RefCell::new(Workspace::new(
            WorkspaceId::new("local", None, "legion", "tmux", ""),
            "legion".into(),
            std::boxed::Box::new(MockRuntime::with_single_pane()),
        )));
        ws.borrow_mut()
            .feed_pane_bytes(PaneId(7), b"alpha TOKEN_BODY one\r\n", 80, 24);
        ws.borrow_mut()
            .feed_pane_bytes(PaneId(8), b"beta\r\n", 80, 24);

        let jumps = Rc::new(RefCell::new(Vec::<(String, u32)>::new()));
        let j = jumps.clone();
        let s = ws.clone();
        show(
            &win,
            PanelShowArgs {
                initial_tab: PanelTab::Search,
                workspaces: vec![],
                attention: vec![],
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
                on_jump_pane: Box::new(move |ws, pane| j.borrow_mut().push((ws, pane))),
                on_send_input: Box::new(|_, _, _| {}),
                on_mute: Box::new(|_, _, _| {}),
                peek_bytes: Box::new(|_, _| (80, 24, Vec::new())),
                search: Box::new(move |query| {
                    s.borrow()
                        .search_workspace(query)
                        .into_iter()
                        .map(SearchRow::from)
                        .collect()
                }),
                on_close: Box::new(|| {}),
            },
        );
        pump_main_loop(80);

        // 空 query：无命中，占位可见。
        let status = find_by_name(&win, "muxterm-search-status").expect("search status 应存在");
        assert!(status.is_visible(), "空 query 应显示占位");

        // 输入 TOKEN_BODY → hit widget 出现，占位隐藏。
        let entry = find_by_name(&win, "muxterm-panel-entry")
            .expect("共享 Entry 应存在")
            .downcast::<gtk4::Entry>()
            .expect("Entry 类型");
        entry.set_text("TOKEN_BODY");
        pump_main_loop(80);

        let hit = find_by_name(&win, "muxterm-search-hit-legion@local-7-1").expect("命中行应存在");
        assert!(hit.is_visible(), "命中行应可见");
        assert!(!status.is_visible(), "有命中时占位应隐藏");

        // 激活 hit → jump Recording 收到 (legion, 7)。
        let list = find_by_name(&win, "muxterm-panel-list")
            .expect("列表应存在")
            .downcast::<gtk4::ListBox>()
            .expect("ListBox 类型");
        let row = list.selected_row().expect("应自动选中首行");
        assert_eq!(row.widget_name(), "muxterm-search-hit-legion@local-7-1");
        let _: () = row.emit_by_name("activate", &[]);
        pump_main_loop(40);
        assert_eq!(
            *jumps.borrow(),
            vec![("legion@local".to_string(), 7)],
            "激活命中行应跳转到 (legion, 7)"
        );

        win.close();
        win.destroy();
        pump_main_loop(40);
    });
}
