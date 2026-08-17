//! W18：搜索范围 = 当前 pane / 本工作区 / 全部已连接工作区；另有 pane 内查找条。
//!
//! 本 crate 只构造一个 AppWindow。core 的 `search_pane` / `search_workspace` /
//! `search_all` 已有；缺的是面板范围开关与 `muxterm-pane-find`。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::feature_e2e_contract::{build_two_pane_cat, FEATURE_TIMEOUT};
use support::linux_gtk::*;
use support::tmux_test_support::{
    kill_server, list_pane_ids, send_keys_line, tmux_available, unique_socket,
    wait_capture_contains,
};

use muxterm::core::config::Config;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

struct ExtraWs {
    socket: String,
    session: String,
    token: String,
}

impl Drop for ExtraWs {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

fn build_extra(label: &str) -> ExtraWs {
    let socket = unique_socket(label);
    let session = format!("scope2-{label}");
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    let token = format!("OTHER_WS_{suffix}");
    let output = std::process::Command::new("tmux")
        .args([
            "-L",
            &socket,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            &session,
            "-x",
            "80",
            "-y",
            "24",
            "--",
            "/bin/cat",
        ])
        .output()
        .expect("new-session");
    assert!(output.status.success(), "extra ws new-session 失败");
    let ids = list_pane_ids(&socket, &session);
    assert_eq!(ids.len(), 1);
    send_keys_line(&socket, &format!("%{}", ids[0]), &token);
    wait_capture_contains(
        &socket,
        &format!("%{}", ids[0]),
        &token,
        Duration::from_secs(3),
    );
    ExtraWs {
        socket,
        session,
        token,
    }
}

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_layout_leaf_ids().is_empty() {
            return true;
        }
    }
    false
}

fn click_named(app: &AppWindow, name: &str) {
    let w = find_by_name(&app.test_window(), name).unwrap_or_else(|| panic!("必须有 {name}"));
    if let Ok(btn) = w.clone().downcast::<gtk4::Button>() {
        let _: () = btn.emit_by_name("clicked", &[]);
        return;
    }
    if let Ok(toggle) = w.downcast::<gtk4::ToggleButton>() {
        toggle.set_active(true);
        return;
    }
    panic!("{name} 应是 Button 或 ToggleButton");
}

fn set_search_query(app: &AppWindow, q: &str) {
    let entry = find_by_name(&app.test_window(), "muxterm-panel-entry")
        .expect("muxterm-panel-entry")
        .downcast::<gtk4::Entry>()
        .expect("Entry");
    entry.set_text(q);
    pump_main_loop(80);
}

fn hit_count(app: &AppWindow) -> usize {
    if find_by_name_prefix(&app.test_window(), "muxterm-search-hit-").is_some() {
        1
    } else {
        0
    }
}

/// 面板三个范围 + 当前 pane 查找条。
#[test]
fn linux_search_scope_pane_workspace_all_and_pane_find() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_two_pane_cat("gtk-scope");
        let extra = build_extra("gtk-scope2");

        let mut cfg = Config::default();
        cfg.tmux.socket = fx.socket.clone();
        cfg.tmux.default_session = fx.session.clone();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);
        assert!(wait_ready(&app), "ws1 attach 后应有 pane");

        app.test_open_spec(WorkspaceSpec::local_tmux(
            Some(extra.session.clone()),
            Some(extra.socket.clone()),
        ));
        let deadline = Instant::now() + FEATURE_TIMEOUT;
        let mut two = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if app.test_workspace_replica_ids().len() >= 2
                && !app.test_search_all(&extra.token).is_empty()
            {
                two = true;
                break;
            }
        }
        assert!(
            two,
            "必须同时连上两个工作区。ids={:?} extra_hits={:?}",
            app.test_workspace_replica_ids(),
            app.test_search_all(&extra.token)
        );

        let ws1 = format!("{}@local", fx.session);
        app.test_activate_workspace(&ws1);
        pump_main_loop(80);
        app.test_poll_once();

        let pane0 = fx.panes[0];
        assert!(
            !app.test_search_pane(pane0, &fx.search_token).is_empty(),
            "API search_pane 应能找到本 pane token"
        );
        assert!(
            app.test_search_pane(pane0, &fx.bg_token).is_empty(),
            "API search_pane 不得落到另一个 pane"
        );
        assert!(
            !app.test_search_workspace(&fx.bg_token).is_empty(),
            "API search_workspace 应覆盖本工作区另一 pane"
        );
        assert!(
            app.test_search_workspace(&extra.token).is_empty(),
            "API search_workspace 不得跨工作区"
        );
        assert!(
            !app.test_search_all(&extra.token).is_empty(),
            "API search_all 必须跨工作区"
        );

        app.test_open_panel(2);
        pump_main_loop(80);
        click_named(&app, "muxterm-search-scope-pane");
        set_search_query(&app, &fx.bg_token);
        assert_eq!(
            hit_count(&app),
            0,
            "范围=当前 pane 时，另一 pane 的 {} 不得出现 muxterm-search-hit-*",
            fx.bg_token
        );
        set_search_query(&app, &fx.search_token);
        assert!(
            hit_count(&app) >= 1,
            "范围=当前 pane 时必须命中 {}",
            fx.search_token
        );

        click_named(&app, "muxterm-search-scope-workspace");
        set_search_query(&app, &fx.bg_token);
        assert!(
            hit_count(&app) >= 1,
            "范围=本工作区必须命中另一 pane 的 {}",
            fx.bg_token
        );
        set_search_query(&app, &extra.token);
        assert_eq!(
            hit_count(&app),
            0,
            "范围=本工作区不得命中另一工作区的 {}",
            extra.token
        );

        click_named(&app, "muxterm-search-scope-all");
        set_search_query(&app, &extra.token);
        assert!(
            hit_count(&app) >= 1,
            "范围=全部已连接必须命中 {}",
            extra.token
        );

        if app.test_panel_open() {
            muxterm::platform::linux::quickconnect_panel::close_current();
            pump_main_loop(40);
        }
        app.test_open_pane_find();
        pump_main_loop(80);
        let bar = find_by_name(&app.test_window(), "muxterm-pane-find")
            .expect("Ctrl+F / test_open_pane_find 必须出现 muxterm-pane-find");
        assert!(bar.is_visible(), "muxterm-pane-find 必须可见");
        let find_entry = find_by_name(&app.test_window(), "muxterm-pane-find-entry")
            .expect("muxterm-pane-find-entry")
            .downcast::<gtk4::Entry>()
            .expect("Entry");
        find_entry.set_text(&fx.search_token);
        pump_main_loop(80);
        assert!(
            app.test_pane_vte_text(pane0).contains(&fx.search_token),
            "pane 内查找后可见区应含 {}",
            fx.search_token
        );
    });
}
