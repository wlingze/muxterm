//! H2 GTK：Herdr attach 的 GTK 路径，断言与本地/SSH attach 一致。
//!
//! 本 crate 只构造一个 AppWindow。禁止 MockRuntime。无 herdr 二进制 skip。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::linux_gtk::*;

use muxterm::core::config::Config;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

const HERDR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_layout_leaf_ids().is_empty() {
            return true;
        }
    }
    false
}

/// 夹具先涂 token 再 GTK attach：VTE 与 search_all 都能看到，且池里
/// 必须是 herdr runtime（不能误开成本地 tmux）。
#[test]
fn linux_herdr_attach_shows_preexist_token() {
    if skip_no_display() {
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let herdr = IsolatedHerdr::start("gtk-att");
        // 真实用户 session 已出现 pP/pQ/pR。构造同样的三 pane，旧实现会
        // 生成重复 L0:L0，并触发 gtk_paned_set_end_child critical。
        let (ws, _tab, [pane_p, _pane_q, _pane_r]) =
            herdr.create_alpha_split_workspace("/tmp", "mux-h2-gtk");
        // 三分屏后最窄 VTE 约 27 列；token 必须保持 HERDR_LIVE_* 且能在
        // 单行完整出现，避免把正常换行误判成字节丢失。
        let token = "HERDR_LIVE_GA";
        herdr.paint(&pane_p, token);

        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        app.test_open_spec(WorkspaceSpec::herdr(
            herdr.name(),
            ws.clone(),
            herdr.socket_path().to_string_lossy().to_string(),
        ));

        assert!(wait_ready(&app), "Herdr attach 后应有 pane");
        let leaves = app.test_layout_leaf_ids();
        let unique: std::collections::HashSet<_> = leaves.iter().copied().collect();
        assert_eq!(leaves.len(), 3, "GTK 必须渲染三个 Herdr pane: {leaves:?}");
        assert_eq!(
            unique.len(),
            3,
            "pP/pQ/pR 必须是三个唯一 VTE，禁止同一 child 挂两次: {leaves:?}"
        );
        assert!(leaves.iter().all(|pane| *pane != 0));
        let deadline = Instant::now() + HERDR_TIMEOUT;
        let mut ok = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if leaves
                .iter()
                .any(|pane| app.test_pane_vte_text(*pane).contains(token))
                && !app.test_search_all(token).is_empty()
            {
                ok = true;
                break;
            }
        }
        assert!(
            ok,
            "Herdr attach 后 VTE 与 search_all 必须含 {token}。vte={:?} search={:?}",
            leaves
                .iter()
                .map(|pane| app.test_pane_vte_text(*pane))
                .collect::<Vec<_>>(),
            app.test_search_all(token)
        );

        // 模拟 VTE commit：逐字输入、再单独 Enter。引号把输入回显中的 token
        // 隔开，所以只有 shell 真执行 echo，VTE/PaneBuf 才可能命中连续 token。
        let command = "echo HERDR_EXEC_\"GTKLOCAL\"";
        let output_token = "HERDR_EXEC_GTKLOCAL";
        assert!(!command.contains(output_token));
        assert!(app.test_search_all(output_token).is_empty());
        for ch in command.chars() {
            assert!(app.test_emit_active_pane_commit(&ch.to_string()));
        }
        assert!(app.test_emit_active_pane_commit("\r"));
        let deadline = Instant::now() + HERDR_TIMEOUT;
        let mut live_ok = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if leaves
                .iter()
                .any(|pane| app.test_pane_vte_text(*pane).contains(output_token))
                && !app.test_search_all(output_token).is_empty()
            {
                live_ok = true;
                break;
            }
        }
        assert!(
            live_ok,
            "GTK 输入必须经 Herdr observe 回到 VTE/PaneBuf。vte={:?} search={:?}",
            leaves
                .iter()
                .map(|pane| app.test_pane_vte_text(*pane))
                .collect::<Vec<_>>(),
            app.test_search_all(output_token)
        );
        assert!(
            app.test_workspace_runtimes().iter().any(|r| r == "herdr"),
            "池里必须是 Herdr 工作区（runtime=herdr），不能误开成本地 tmux。runtimes={:?}",
            app.test_workspace_runtimes()
        );
    });
}
