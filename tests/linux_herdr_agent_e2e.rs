//! Herdr agent lifecycle 的 GTK 端到端契约。
//!
//! Herdr wire 只由 Runtime 解析；AppWindow 只消费通用 PaneAgentChanged →
//! AttentionSignal。夹具使用真实 named session 和真实可检测的 pi 进程。

#![cfg(feature = "gtk")]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::herdr_test_support::{herdr_available, IsolatedHerdr, TempAgentCommand};
use support::linux_gtk::*;

use muxterm::core::config::Config;
use muxterm::core::runtime::herdr::session::HerdrSession;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

const HERDR_TIMEOUT: Duration = Duration::from_secs(15);

fn pump_until(app: &AppWindow, predicate: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if predicate() {
            return true;
        }
    }
    false
}

#[test]
fn linux_herdr_agent_transitions_drive_generic_attention_once() {
    if skip_no_display() {
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }

    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let agent_command = TempAgentCommand::pi("gtk-agent");
        let herdr = IsolatedHerdr::start("gtk-agent");
        let (workspace_id, agent_tab_id, pane_id) = herdr.create_workspace(
            agent_command
                .cwd()
                .to_str()
                .expect("临时 agent cwd 不是 UTF-8"),
            "mux-agent-gtk",
        );
        let session = Arc::new(HerdrSession::new(herdr.name(), herdr.socket_path()));
        let source = "herdr:pi";
        let agent_session_path = agent_command.cwd().join("pi-session.jsonl");
        std::fs::write(&agent_session_path, "{}\n").expect("创建临时 pi session 失败");

        herdr.paint(&pane_id, agent_command.invocation());
        let detected_deadline = Instant::now() + HERDR_TIMEOUT;
        loop {
            let snapshot = session.snapshot().expect("读取 agent detection snapshot");
            if snapshot
                .agents
                .iter()
                .any(|agent| agent.pane_id == pane_id && agent.agent.as_deref() == Some("pi"))
            {
                break;
            }
            assert!(
                Instant::now() < detected_deadline,
                "Herdr 未识别 GTK 夹具里的真实 pi: {snapshot:#?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }

        session
            .call(
                "pane.report_agent_session",
                serde_json::json!({
                    "pane_id": pane_id,
                    "source": source,
                    "agent": "pi",
                    "seq": 1,
                    "agent_session_path": agent_session_path,
                    "session_start_source": "startup",
                }),
            )
            .expect("报告 GTK pi session");
        session
            .call(
                "pane.report_agent",
                serde_json::json!({
                    "pane_id": pane_id,
                    "source": source,
                    "agent": "pi",
                    "state": "working",
                    "message": "running GTK lifecycle",
                    "seq": 2,
                    "agent_session_path": agent_session_path,
                }),
            )
            .expect("报告 GTK working");
        session
            .call(
                "tab.create",
                serde_json::json!({
                    "workspace_id": &workspace_id,
                    "label": "foreground-shell",
                    "focus": true,
                }),
            )
            .expect("创建前台 shell tab，让 agent pane 成为后台");

        let app = AppWindow::new(Config::default(), load_theme());
        app.window.set_default_size(1000, 700);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(100);
        app.test_open_spec(WorkspaceSpec::herdr(
            herdr.name(),
            &workspace_id,
            herdr.socket_path().to_string_lossy(),
        ));
        assert!(
            pump_until(&app, || !app.test_layout_leaf_ids().is_empty()),
            "GTK Herdr agent workspace 应 attach"
        );
        assert_eq!(app.test_attention_blocked_workspaces(), 0);
        assert_eq!(app.test_attention_done_count(), 0);
        assert!(
            app.test_notifications_recorded().is_empty(),
            "attach 的初始 working 状态不应伪造通知"
        );

        session
            .call(
                "pane.report_agent",
                serde_json::json!({
                    "pane_id": pane_id,
                    "source": source,
                    "agent": "pi",
                    "state": "blocked",
                    "message": "approve GTK command",
                    "seq": 3,
                    "agent_session_path": agent_session_path,
                }),
            )
            .expect("报告 GTK blocked");
        assert!(
            pump_until(&app, || {
                app.test_attention_blocked_workspaces() == 1
                    && app
                        .test_notifications_recorded()
                        .iter()
                        .any(|line| line.ends_with(": needs attention"))
            }),
            "通用 PaneAgentChanged(blocked) 必须更新 GTK attention 与通知: {:?}",
            app.test_notifications_recorded()
        );
        for _ in 0..10 {
            app.test_poll_once();
            pump_main_loop(10);
        }
        assert_eq!(
            app.test_notifications_recorded()
                .iter()
                .filter(|line| line.ends_with(": needs attention"))
                .count(),
            1,
            "blocked 保持期间只能通知一次"
        );

        // Done 不是 pane.report_agent 可写入的状态；释放 full-lifecycle hook
        // 后，让真实后台 pi 画出 Herdr 的完成帧，由 Herdr 自己推导 Done。
        session
            .call(
                "pane.clear_agent_authority",
                serde_json::json!({
                    "pane_id": pane_id,
                    "source": source,
                    "seq": 4,
                }),
            )
            .expect("清除 GTK agent hook authority");
        let detector_deadline = Instant::now() + HERDR_TIMEOUT;
        loop {
            let snapshot = session
                .snapshot()
                .expect("release 后读取 detector snapshot");
            if snapshot.agents.iter().any(|agent| {
                agent.pane_id == pane_id
                    && agent.agent_status
                        == muxterm::core::runtime::herdr::session::HerdrAgentStatus::Working
                    && !agent.screen_detection_skipped
            }) {
                break;
            }
            assert!(
                Instant::now() < detector_deadline,
                "release 后 screen detector 未接管为 Working: {snapshot:#?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        agent_command.mark_done();
        let done_reached = pump_until(&app, || {
            app.test_attention_done_count() == 1
                && app
                    .test_notifications_recorded()
                    .iter()
                    .any(|line| line.ends_with(": task complete"))
        });
        let done_snapshot = session.snapshot().expect("Done 后读取诊断 snapshot");
        assert!(
            done_reached,
            "通用 PaneAgentChanged(done) 必须更新 GTK attention 与通知: {:?}; snapshot={done_snapshot:#?}",
            app.test_notifications_recorded()
        );
        for _ in 0..10 {
            app.test_poll_once();
            pump_main_loop(10);
        }
        assert_eq!(
            app.test_notifications_recorded()
                .iter()
                .filter(|line| line.ends_with(": task complete"))
                .count(),
            1,
            "done 保持期间只能通知一次"
        );

        agent_command.stop();
        session
            .call("tab.focus", serde_json::json!({ "tab_id": &agent_tab_id }))
            .expect("聚焦完成 pane 应把 Done 清为 Idle");
        assert!(
            pump_until(&app, || {
                app.test_attention_blocked_workspaces() == 0 && app.test_attention_done_count() == 0
            }),
            "release 后必须清除权威状态并恢复通用 attention"
        );
    });
}
