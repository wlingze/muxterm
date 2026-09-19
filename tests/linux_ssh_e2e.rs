//! W18：loopback SSH attach 的 GTK 路径，断言与本地 attach 一致。
//!
//! 本 crate 只构造一个 AppWindow。禁止 MockRuntime。无 sshd 二进制 skip。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::linux_gtk::*;
use support::ssh_tmux_contract::{build_remote_one_pane, ssh_tmux_available, SSH_TIMEOUT};
use support::tmux_test_support::{
    create_session, list_pane_ids, send_keys_line, tmux_available, TmuxServerGuard,
};

use muxterm::test_support::core::config::Config;
use muxterm::test_support::frontend::linux::window::AppWindow;
use muxterm::test_support::frontend::utils::corebridge::{
    ClientCandidateRef, ClientExistingCandidateRef, ClientOpenIntent, ClientOpenRequest,
};

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + SSH_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_layout_leaf_ids().is_empty() {
            return true;
        }
    }
    false
}

/// SSH attach 已有 `/bin/cat` 画面：VTE 与 search_all 都能看到 token。
#[test]
fn linux_ssh_attach_shows_preexist_token() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    if !ssh_tmux_available() {
        eprintln!("skip: 无 sshd 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_remote_one_pane("gtk-ssh-att");
        fx.apply_ssh_config_env();

        let local = TmuxServerGuard::new("gtk-ssh-identity-local");
        create_session(local.socket(), "identity-local", 100, 30);
        let local_pane = list_pane_ids(local.socket(), "identity-local")[0];
        assert_eq!(
            local_pane, fx.pane,
            "fixture must exercise duplicate numeric pane IDs"
        );
        let local_token = "LOCAL_IDENTITY_SEARCH_TOKEN";
        send_keys_line(
            local.socket(),
            &format!("%{local_pane}"),
            &format!("printf '{local_token}\\n'"),
        );
        let mut cfg = Config::default();
        cfg.tmux.socket = local.socket().into();
        cfg.tmux.default_session = "identity-local".into();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);
        let local_workspace = app.test_active_workspace_replica_id();
        send_keys_line(
            local.socket(),
            &format!("%{local_pane}"),
            &format!("printf '{local_token}\\n'"),
        );
        let local_deadline = Instant::now() + SSH_TIMEOUT;
        while app.test_search_all(local_token).is_empty() && Instant::now() < local_deadline {
            pump_main_loop(30);
        }
        assert!(
            !app.test_search_all(local_token).is_empty(),
            "local fixture must be indexed before switching: {:?}",
            app.test_pane_vte_buffer_text(local_pane)
        );

        std::env::set_var("MUXTERM_TEST_REMOTE_TMUX_SOCKET", &fx.socket);
        app.test_start_open(ClientOpenRequest {
            candidate: ClientCandidateRef::Existing {
                identity: ClientExistingCandidateRef {
                    runtime_id: "tmux".into(),
                    transport_id: "ssh".into(),
                    target: fx.sshd.alias.clone(),
                    session: Some(fx.session.clone()),
                    socket: Some(fx.socket.clone()),
                    workspace_id: None,
                },
            },
            intent: ClientOpenIntent::AttachOnly,
            template: None,
            activate: true,
        });
        let open_deadline = Instant::now() + SSH_TIMEOUT;
        while app.test_open_pending() && Instant::now() < open_deadline {
            pump_main_loop(20);
        }
        assert!(!app.test_open_pending(), "SSH asynchronous open deadline");
        assert_ne!(
            app.test_active_workspace_replica_id(),
            local_workspace,
            "{:?}",
            app.test_notifications_recorded()
        );

        assert!(wait_ready(&app), "SSH attach 后应有 pane");
        let pane = *app.test_layout_leaf_ids().first().expect("pane");
        let deadline = Instant::now() + SSH_TIMEOUT;
        let mut ok = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if app.test_pane_vte_text(pane).contains(&fx.token)
                && !app.test_search_all(&fx.token).is_empty()
            {
                ok = true;
                break;
            }
        }
        assert!(
            ok,
            "SSH attach 后 VTE 与 search_all 必须含 {}（与本地 linux_feature_e2e 同级）。vte={:?} search={:?}",
            fx.token,
            app.test_pane_vte_text(pane),
            app.test_search_all(&fx.token)
        );
        assert!(
            app.test_workspace_replica_ids()
                .iter()
                .any(|id| id.contains(&fx.sshd.alias)),
            "池里必须是 SSH 工作区（replica 含 loopback alias），不能误开成本地。ids={:?}",
            app.test_workspace_replica_ids()
        );
        let remote_workspace = app.test_active_workspace_replica_id();
        for (token, expected) in [
            (local_token, &local_workspace),
            (fx.token.as_str(), &remote_workspace),
        ] {
            let deadline = Instant::now() + SSH_TIMEOUT;
            while app.test_search_all(token).is_empty() && Instant::now() < deadline {
                pump_main_loop(30);
            }
            let hits = app.test_search_all(token);
            assert!(!hits.is_empty(), "missing indexed token {token}");
            assert!(hits
                .iter()
                .all(|(workspace, id, _)| workspace == expected && *id == local_pane));
            app.test_open_panel(2);
            pump_main_loop(80);
            find_by_name(&app.window, "muxterm-panel-entry")
                .unwrap()
                .downcast::<gtk4::Entry>()
                .unwrap()
                .set_text(token);
            pump_main_loop(150);
            let row = find_by_name_prefix(&app.window, "muxterm-search-hit-")
                .expect("search result")
                .downcast::<gtk4::ListBoxRow>()
                .unwrap();
            row.activate();
            pump_main_loop(150);
            assert_eq!(
                &app.test_active_workspace_replica_id(),
                expected,
                "search must resolve full workspace identity"
            );
            assert!(!app.test_panel_open());
        }
        app.shutdown();
        pump_main_loop(100);
    });
}
