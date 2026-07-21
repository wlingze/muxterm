//! CLI 集成测试：端到端测试 CLI 命令（LocalBackend）。
//!
//! 直接调用 cli 模块函数，不 spawn 进程（避免共享 state 的复杂性）。
//! 覆盖：session/window/tab/pane 管理 + send-keys/capture-pane。

#![cfg(feature = "tui")]

use muxterm::cli::{format_output, parse_cli_command, CliCommand, OutputFormat};
use muxterm::core::backend::LocalBackend;
use muxterm::core::model::task::Task;
use muxterm::core::model::TerminalModel;
use muxterm::core::types::{PaneId, TabId, WindowId};

fn make_model() -> TerminalModel {
    let backend = LocalBackend::new("sleep", "/");
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    model
}

fn exec_and_drain(model: &mut TerminalModel, task: Task) {
    model.execute(task).unwrap();
    let _ = model.poll_events();
}

// ── Session 测试 ──────────────────────────────────────────

#[test]
fn cli_list_sessions_json() {
    let model = make_model();
    let out = format_output(model.state(), &CliCommand::ListSessions, OutputFormat::Json);
    assert!(out.contains(r#""name":"local""#));
    assert!(out.contains(r#""attached":true"#));
}

#[test]
fn cli_list_sessions_text() {
    let model = make_model();
    let out = format_output(model.state(), &CliCommand::ListSessions, OutputFormat::Text);
    assert!(out.contains("local"));
    assert!(out.contains("attached"));
}

// ── Window 测试 ───────────────────────────────────────────

#[test]
fn cli_list_windows() {
    let model = make_model();
    let out = format_output(
        model.state(),
        &CliCommand::ListWindows { session: None },
        OutputFormat::Text,
    );
    assert!(out.contains("w1"));
}

#[test]
fn cli_new_window_and_list() {
    let mut model = make_model();
    exec_and_drain(
        &mut model,
        Task::NewWindow {
            name: Some("dev".into()),
            command: None,
            workdir: None,
        },
    );
    let out = format_output(
        model.state(),
        &CliCommand::ListWindows { session: None },
        OutputFormat::Text,
    );
    assert!(out.contains("w2"));
    assert!(out.contains("dev"));
}

#[test]
fn cli_kill_window() {
    let mut model = make_model();
    exec_and_drain(
        &mut model,
        Task::NewWindow {
            name: None,
            command: None,
            workdir: None,
        },
    );
    assert_eq!(
        model.state().active_window().map(|w| w.id),
        Some(WindowId(2))
    );
    exec_and_drain(
        &mut model,
        Task::CloseWindow {
            target: WindowId(2),
        },
    );
    assert_eq!(
        model.state().active_window().map(|w| w.id),
        Some(WindowId(1))
    );
}

#[test]
fn cli_select_window() {
    let mut model = make_model();
    exec_and_drain(
        &mut model,
        Task::NewWindow {
            name: None,
            command: None,
            workdir: None,
        },
    );
    exec_and_drain(
        &mut model,
        Task::SwitchWindow {
            target: WindowId(1),
        },
    );
    assert_eq!(
        model.state().active_window().map(|w| w.id),
        Some(WindowId(1))
    );
}

#[test]
fn cli_rename_window() {
    let mut model = make_model();
    exec_and_drain(
        &mut model,
        Task::RenameWindow {
            target: WindowId(1),
            name: "renamed".into(),
        },
    );
    assert_eq!(model.state().active_window().unwrap().name, "renamed");
}

// ── Tab 测试 ──────────────────────────────────────────────

#[test]
fn cli_new_tab_and_list() {
    let mut model = make_model();
    let out_before = format_output(
        model.state(),
        &CliCommand::ListTabs { window: None },
        OutputFormat::Text,
    );
    assert!(out_before.contains("t1"));

    exec_and_drain(
        &mut model,
        Task::NewTab {
            window: WindowId(1),
            name: Some("logs".into()),
            command: None,
            workdir: None,
        },
    );
    let out_after = format_output(
        model.state(),
        &CliCommand::ListTabs { window: None },
        OutputFormat::Text,
    );
    assert!(out_after.contains("t2"));
    assert!(out_after.contains("logs"));
}

#[test]
fn cli_kill_tab() {
    let mut model = make_model();
    exec_and_drain(
        &mut model,
        Task::NewTab {
            window: WindowId(1),
            name: None,
            command: None,
            workdir: None,
        },
    );
    assert!(model.state().tabs(&WindowId(1)).len() >= 2);
    exec_and_drain(&mut model, Task::CloseTab { target: TabId(2) });
    assert_eq!(model.state().tabs(&WindowId(1)).len(), 1);
}

#[test]
fn cli_select_tab() {
    let mut model = make_model();
    exec_and_drain(
        &mut model,
        Task::NewTab {
            window: WindowId(1),
            name: None,
            command: None,
            workdir: None,
        },
    );
    exec_and_drain(&mut model, Task::SwitchTab { target: TabId(1) });
    assert_eq!(model.state().active_tab().map(|t| t.id), Some(TabId(1)));
}

#[test]
fn cli_rename_tab() {
    let mut model = make_model();
    exec_and_drain(
        &mut model,
        Task::RenameTab {
            target: TabId(1),
            name: "shell".into(),
        },
    );
    assert_eq!(model.state().active_tab().unwrap().name, "shell");
}

// ── Pane 测试 ─────────────────────────────────────────────

#[test]
fn cli_split_pane_and_list() {
    let mut model = make_model();
    let pane = model.state().active_pane().unwrap().id;
    exec_and_drain(
        &mut model,
        Task::SplitPane {
            target: Some(pane),
            dir: muxterm::core::model::layout::SplitDir::Horizontal,
            command: None,
            workdir: None,
        },
    );
    let out = format_output(
        model.state(),
        &CliCommand::ListPanes { tab: None },
        OutputFormat::Json,
    );
    assert!(out.contains(r#""id":"@1""#));
    assert!(out.contains(r#""id":"@2""#));
}

#[test]
fn cli_split_pane_text_format() {
    let mut model = make_model();
    let pane = model.state().active_pane().unwrap().id;
    exec_and_drain(
        &mut model,
        Task::SplitPane {
            target: Some(pane),
            dir: muxterm::core::model::layout::SplitDir::Vertical,
            command: None,
            workdir: None,
        },
    );
    let out = format_output(
        model.state(),
        &CliCommand::ListPanes { tab: None },
        OutputFormat::Text,
    );
    assert!(out.contains("@1"));
    assert!(out.contains("@2"));
}

#[test]
fn cli_kill_pane() {
    let mut model = make_model();
    let pane = model.state().active_pane().unwrap().id;
    exec_and_drain(
        &mut model,
        Task::SplitPane {
            target: Some(pane),
            dir: muxterm::core::model::layout::SplitDir::Horizontal,
            command: None,
            workdir: None,
        },
    );
    let new_pane = model.state().active_pane().unwrap().id;
    assert_ne!(pane, new_pane);
    exec_and_drain(&mut model, Task::ClosePane { target: new_pane });
    assert_eq!(model.state().active_pane().map(|p| p.id), Some(pane));
}

#[test]
fn cli_select_pane() {
    let mut model = make_model();
    let pane1 = model.state().active_pane().unwrap().id;
    exec_and_drain(
        &mut model,
        Task::SplitPane {
            target: Some(pane1),
            dir: muxterm::core::model::layout::SplitDir::Horizontal,
            command: None,
            workdir: None,
        },
    );
    let pane2 = model.state().active_pane().unwrap().id;
    exec_and_drain(&mut model, Task::SwitchPane { target: pane1 });
    assert_eq!(model.state().active_pane().map(|p| p.id), Some(pane1));
    exec_and_drain(&mut model, Task::SwitchPane { target: pane2 });
    assert_eq!(model.state().active_pane().map(|p| p.id), Some(pane2));
}

#[test]
fn cli_resize_pane() {
    let mut model = make_model();
    let pane = model.state().active_pane().unwrap().id;
    exec_and_drain(
        &mut model,
        Task::ResizePane {
            target: pane,
            cols: 120,
            rows: 40,
        },
    );
    let p = model.state().pane(&pane).unwrap();
    assert_eq!(p.cols, 120);
    assert_eq!(p.rows, 40);
}

// ── 输入输出测试 ──────────────────────────────────────────

#[test]
fn cli_send_keys_and_capture() {
    let mut model = make_model();
    let pane = model.state().active_pane().unwrap().id;
    // WriteRaw 更直接，不经过编码
    exec_and_drain(
        &mut model,
        Task::WriteRaw {
            target: pane,
            data: b"hello cli\n".to_vec(),
        },
    );
    let out = format_output(
        model.state(),
        &CliCommand::CapturePane {
            target: Some(pane),
            lines: None,
        },
        OutputFormat::Text,
    );
    assert!(out.contains("hello cli"));
}

#[test]
fn cli_capture_pane_with_lines_limit() {
    use muxterm::core::model::backend::mock::MockBackend;
    use muxterm::core::model::TerminalModel;
    let mut b = MockBackend::with_single_pane();
    b.outputs[0].1 = b"alpha\nbeta\ngamma\ndelta\n".to_vec();
    let model = TerminalModel::new(Box::new(b));
    let out = format_output(
        model.state(),
        &CliCommand::CapturePane {
            target: Some(PaneId(1)),
            lines: Some(2),
        },
        OutputFormat::Text,
    );
    assert!(out.contains("delta"), "应含 delta: {out}");
    assert!(!out.contains("alpha"), "不应含 alpha: {out}");
}

// ── 布局查询测试 ──────────────────────────────────────────

#[test]
fn cli_list_layout_text() {
    let model = make_model();
    let out = format_output(
        model.state(),
        &CliCommand::ListLayout { window: None },
        OutputFormat::Text,
    );
    assert!(out.contains("window"));
    assert!(out.contains("tab"));
    assert!(out.contains("@1"));
}

#[test]
fn cli_list_layout_json() {
    let model = make_model();
    let out = format_output(
        model.state(),
        &CliCommand::ListLayout { window: None },
        OutputFormat::Json,
    );
    assert!(out.contains(r#""id":"t1""#));
}

#[test]
fn cli_list_layout_after_split() {
    let mut model = make_model();
    let pane = model.state().active_pane().unwrap().id;
    exec_and_drain(
        &mut model,
        Task::SplitPane {
            target: Some(pane),
            dir: muxterm::core::model::layout::SplitDir::Horizontal,
            command: None,
            workdir: None,
        },
    );
    let out = format_output(
        model.state(),
        &CliCommand::ListLayout { window: None },
        OutputFormat::Text,
    );
    assert!(out.contains("@1"));
    assert!(out.contains("@2"));
}

// ── display-message 测试 ──────────────────────────────────

#[test]
fn cli_display_message_pane_id() {
    let model = make_model();
    let pane = model.state().active_pane().unwrap().id;
    let out = format_output(
        model.state(),
        &CliCommand::DisplayMessage {
            target: pane,
            format: "#{pane_id}".into(),
        },
        OutputFormat::Text,
    );
    assert_eq!(out, format!("@{}", pane.0));
}

#[test]
fn cli_display_message_pane_size() {
    let model = make_model();
    let pane = model.state().active_pane().unwrap().id;
    let out = format_output(
        model.state(),
        &CliCommand::DisplayMessage {
            target: pane,
            format: "#{pane_width}x#{pane_height}".into(),
        },
        OutputFormat::Text,
    );
    let p = model.state().pane(&pane).unwrap();
    assert_eq!(out, format!("{}x{}", p.cols, p.rows));
}

// ── 命令解析集成测试 ──────────────────────────────────────

#[test]
fn cli_parse_and_execute_split() {
    let (cmd, _) =
        parse_cli_command(&["split-pane".into(), "-h".into(), "-t".into(), "@1".into()]).unwrap();
    assert!(matches!(
        cmd,
        CliCommand::SplitPane {
            horizontal: true,
            ..
        }
    ));
}

#[test]
fn cli_parse_send_keys_with_text() {
    let (cmd, _) = parse_cli_command(&[
        "send-keys".into(),
        "-t".into(),
        "@1".into(),
        "echo test".into(),
    ])
    .unwrap();
    match cmd {
        CliCommand::SendKeys { target, text } => {
            assert_eq!(target, Some(PaneId(1)));
            assert_eq!(text, "echo test");
        }
        _ => panic!(),
    }
}

// ── TmuxBackend 测试 ──────────────────────────────────────

#[test]
fn cli_tmux_backend_connect_and_list() {
    use muxterm::core::backend::TmuxBackend;
    use std::time::{SystemTime, UNIX_EPOCH};

    let socket = format!(
        "cli-tmux-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );

    let backend = TmuxBackend::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = rt.block_on(model.connect());
    if result.is_err() {
        eprintln!("skip: tmux 不可用");
        let _ = std::process::Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .status();
        return;
    }
    let _ = model.poll_events();

    let out = format_output(model.state(), &CliCommand::ListSessions, OutputFormat::Text);
    assert!(!out.is_empty(), "应列出 session");

    let _ = rt.block_on(model.shutdown());
    let _ = std::process::Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .status();
}

#[test]
fn cli_tmux_backend_new_window() {
    use muxterm::core::backend::TmuxBackend;
    use std::time::{SystemTime, UNIX_EPOCH};

    let socket = format!(
        "cli-tmux-nw-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );

    let backend = TmuxBackend::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    if rt.block_on(model.connect()).is_err() {
        eprintln!("skip: tmux 不可用");
        let _ = std::process::Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .status();
        return;
    }
    let _ = model.poll_events();

    let initial = model.state().sessions().len();
    model
        .execute(Task::NewWindow {
            name: Some("test".into()),
            command: None,
            workdir: None,
        })
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = model.poll_events();

    assert!(
        model.state().sessions().len() >= initial,
        "新 window 应建立"
    );

    let _ = rt.block_on(model.shutdown());
    let _ = std::process::Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .status();
}

#[test]
fn cli_tmux_backend_send_keys() {
    use muxterm::core::backend::TmuxBackend;
    use std::time::{SystemTime, UNIX_EPOCH};

    let socket = format!(
        "cli-tmux-sk-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );

    let backend = TmuxBackend::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    if rt.block_on(model.connect()).is_err() {
        eprintln!("skip: tmux 不可用");
        let _ = std::process::Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .status();
        return;
    }
    let _ = model.poll_events();

    let pane = model
        .state()
        .active_pane()
        .map(|p| p.id)
        .unwrap_or(PaneId(0));
    let outcome = model
        .execute(Task::SendKeys {
            target: pane,
            keys: vec![muxterm::core::terminal::input::KeyEvent::Char('x')],
        })
        .unwrap();
    assert!(matches!(
        outcome,
        muxterm::core::model::task::TaskOutcome::Done
    ));

    let _ = rt.block_on(model.shutdown());
    let _ = std::process::Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .status();
}
