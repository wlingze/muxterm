//! 隐藏 workspace 的 Control/Activity lane 仍持续更新前端 badge。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;

use muxterm::test_support::core::config::Config;
use muxterm::test_support::core::workspace::spec::WorkspaceSpec;
use muxterm::test_support::platform::linux::window::AppWindow;

use support::linux_gtk::*;
use support::ssh_tmux_contract::{build_remote_one_pane, ssh_tmux_available, SSH_TIMEOUT};
use support::tmux_test_support::tmux_available;

#[test]
fn hidden_ssh_workspace_activity_updates_badge_while_local_scene_is_visible() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() || !ssh_tmux_available() {
        eprintln!("skip: 无 tmux 或 sshd 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_remote_one_pane("gtk-hidden-activity");
        fx.apply_ssh_config_env();

        let mut cfg = Config::default();
        cfg.attention.blocked_regex = vec!["NEED_INPUT".into()];
        cfg.attention.debounce_ms = 50;
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        let local_replica = app.test_active_workspace_replica_id();
        app.test_open_spec(WorkspaceSpec::ssh_tmux(
            fx.sshd.alias.clone(),
            Some(fx.session.clone()),
            Some(fx.socket.clone()),
        ));

        let pane_deadline = Instant::now() + SSH_TIMEOUT;
        while Instant::now() < pane_deadline && app.test_layout_leaf_ids().is_empty() {
            app.test_poll_once();
            pump_main_loop(30);
        }
        assert!(
            !app.test_layout_leaf_ids().is_empty(),
            "SSH attach 后应有 pane"
        );
        let ssh_pane = *app.test_layout_leaf_ids().first().expect("SSH pane");
        let seed_deadline = Instant::now() + SSH_TIMEOUT;
        let mut seeded = false;
        while Instant::now() < seed_deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if app.test_pane_vte_text(ssh_pane).contains(&fx.token)
                && !app.test_search_all(&fx.token).is_empty()
            {
                seeded = true;
                break;
            }
        }
        assert!(
            seeded,
            "SSH attach 必须先完成初始 snapshot/Index seed，vte={:?}, search={:?}",
            app.test_pane_vte_text(ssh_pane),
            app.test_search_all(&fx.token)
        );
        let ssh_replica = app
            .test_workspace_replica_ids()
            .into_iter()
            .find(|replica| replica.contains(&fx.sshd.alias))
            .expect("池里必须有 SSH workspace");

        // 只切换 frontend-visible Scene；Core 不需要被激活，SSH workspace 现在是隐藏的。
        app.test_activate_workspace(&local_replica);
        pump_main_loop(120);
        assert_eq!(app.test_active_workspace_replica_id(), local_replica);

        // 真实远端输出走 Runtime/Activity lane，不依赖兼容注入路径。
        fx.send_keys_line("NEED_INPUT");
        assert!(
            fx.capture().contains("NEED_INPUT"),
            "远端夹具必须先确认输出已写入 pane: {:?}",
            fx.capture()
        );
        let deadline = Instant::now() + SSH_TIMEOUT;
        let mut blocked = false;
        let mut polls = 0usize;
        let mut output_events = 0usize;
        while Instant::now() < deadline {
            polls += 1;
            output_events += app.test_poll_output_event_count();
            pump_main_loop(30);
            if app.test_attention_blocked_workspaces() == 1 {
                blocked = true;
                break;
            }
        }
        let diagnostic_vte = if blocked {
            String::new()
        } else {
            app.test_activate_workspace(&ssh_replica);
            app.test_flush_feeds();
            let text = app.test_pane_vte_text(ssh_pane);
            app.test_activate_workspace(&local_replica);
            text
        };
        assert!(
            blocked,
            "隐藏 SSH workspace 的匹配输出必须进入 Activity lane，polls={polls}, output_events={output_events}, ssh_vte={diagnostic_vte:?}, snapshot={:?}, search={:?}",
            app.test_attention_snapshot(),
            app.test_search_all("NEED_INPUT")
        );
        assert_eq!(
            app.test_active_workspace_replica_id(),
            local_replica,
            "隐藏 workspace 的 Activity 更新不得改变当前 Scene"
        );

        let notify = find_by_name(&app.test_window(), "muxterm-status-notify")
            .expect("notification badge should exist")
            .downcast::<gtk4::Button>()
            .expect("notification badge should be a button");
        assert!(
            notify
                .label()
                .map(|label| label.to_string())
                .unwrap_or_default()
                .contains('1'),
            "hidden workspace Activity must update the visible badge: {:?}",
            notify.label()
        );

        // Keep the discovered identity in the assertion path so a future test
        // fixture cannot silently pass with only the startup local workspace.
        assert!(ssh_replica.contains(&fx.sshd.alias));
        app.shutdown();
        pump_main_loop(120);
    });
}
