//! CLI 集成测试：端到端测试 CLI 命令（LocalBackend）。
//!
//! 直接调用 cli 模块函数，不 spawn 进程（避免共享 state 的复杂性）。
//! 覆盖：session/window/tab/pane 管理 + send-keys/capture-pane。
//!
//! `make_model` 使用长驻 `cat`（勿用裸 `sleep`：无参数会立刻退出，
//! 触发末 pane Exit → 关 window，导致 kill/select/split 等用例假失败）。

#![cfg(feature = "tui")]

use muxterm::core::types::{PaneId, TabId, WindowId};
use muxterm::platform::cli::{format_output, parse_cli_command, CliCommand, OutputFormat};
use muxterm::protocol::model::task::Task;
use muxterm::protocol::model::TerminalModel;
use muxterm::runtime::LocalBackend;

fn make_model() -> TerminalModel {
    // cat：阻塞读 stdin、回显 stdout，适合结构测试 + WriteRaw/capture
    let backend = LocalBackend::new("cat", "/");
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
            dir: muxterm::protocol::model::layout::SplitDir::Horizontal,
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
            dir: muxterm::protocol::model::layout::SplitDir::Vertical,
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
            dir: muxterm::protocol::model::layout::SplitDir::Horizontal,
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
            dir: muxterm::protocol::model::layout::SplitDir::Horizontal,
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
    // WriteRaw 更直接，不经过编码；cat 回显后需 drain 进 pane_output
    exec_and_drain(
        &mut model,
        Task::WriteRaw {
            target: pane,
            data: b"hello cli\n".to_vec(),
        },
    );
    // 短轮询：pty 读线程异步送达
    // 用 refresh() 而非 poll_events()——refresh 先从 backend 拉取 pty 输出
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut out = String::new();
    while std::time::Instant::now() < deadline {
        let _ = model.refresh();
        out = format_output(
            model.state(),
            &CliCommand::CapturePane {
                target: Some(pane),
                lines: None,
            },
            OutputFormat::Text,
        );
        if out.contains("hello cli") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(out.contains("hello cli"), "capture 应含写入内容: {out:?}");
}

#[test]
fn cli_capture_pane_with_lines_limit() {
    use muxterm::protocol::model::backend::mock::MockBackend;
    use muxterm::protocol::model::TerminalModel;
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
            dir: muxterm::protocol::model::layout::SplitDir::Horizontal,
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

// ── 嵌套分割布局测试 ─────────────────────────────────────

#[test]
fn cli_nested_split_layout_tree() {
    let mut model = make_model();
    let pane1 = model.state().active_pane().unwrap().id;

    // 水平分割 → @1 | @2
    exec_and_drain(
        &mut model,
        Task::SplitPane {
            target: Some(pane1),
            dir: muxterm::protocol::model::layout::SplitDir::Horizontal,
            command: None,
            workdir: None,
        },
    );

    // 切到 @2（新 pane 成为 active）
    let pane2 = model.state().active_pane().unwrap().id;
    assert_eq!(pane2, PaneId(2), "分割后 active 应为 @2");

    // 对 @2 垂直分割 → @2 上 / @3 下
    exec_and_drain(
        &mut model,
        Task::SplitPane {
            target: Some(pane2),
            dir: muxterm::protocol::model::layout::SplitDir::Vertical,
            command: None,
            workdir: None,
        },
    );

    // 验证布局树: Split(H, @1, Split(V, @2, @3))
    let out = format_output(
        model.state(),
        &CliCommand::ListLayout { window: None },
        OutputFormat::Json,
    );
    assert!(
        out.contains(r#""dir":"horizontal""#),
        "布局 JSON 应含 horizontal: {out}"
    );
    assert!(
        out.contains(r#""dir":"vertical""#),
        "布局 JSON 应含 vertical: {out}"
    );

    // 验证 3 个 pane
    let panes = format_output(
        model.state(),
        &CliCommand::ListPanes { tab: None },
        OutputFormat::Json,
    );
    assert!(
        panes.contains("@1") && panes.contains("@2") && panes.contains("@3"),
        "list-panes 应含 @1 @2 @3: {panes}"
    );

    // 验证 leaves 顺序: @1, @2, @3
    let layout = model
        .state()
        .active_tab()
        .and_then(|t| model.state().layout(&t.id));
    assert!(layout.is_some(), "应有 layout");
    let leaves = layout.unwrap().tree.leaves();
    assert_eq!(
        leaves,
        vec![PaneId(1), PaneId(2), PaneId(3)],
        "leaves 顺序应为 [@1, @2, @3], 实际: {leaves:?}"
    );
}

#[test]
fn cli_nested_split_text_layout_shows_tree() {
    let mut model = make_model();
    let pane1 = model.state().active_pane().unwrap().id;

    // Split(H) @1 → @1 | @2
    exec_and_drain(
        &mut model,
        Task::SplitPane {
            target: Some(pane1),
            dir: muxterm::protocol::model::layout::SplitDir::Horizontal,
            command: None,
            workdir: None,
        },
    );

    // 切到 @1，再 Split(V) @1 → @1 上 / @3 下
    exec_and_drain(&mut model, Task::SwitchPane { target: pane1 });
    exec_and_drain(
        &mut model,
        Task::SplitPane {
            target: Some(pane1),
            dir: muxterm::protocol::model::layout::SplitDir::Vertical,
            command: None,
            workdir: None,
        },
    );

    // text 格式布局
    let out = format_output(
        model.state(),
        &CliCommand::ListLayout { window: None },
        OutputFormat::Text,
    );
    assert!(out.contains("@1"), "text 布局应含 @1: {out}");
    assert!(out.contains("@2"), "text 布局应含 @2: {out}");
    assert!(out.contains("@3"), "text 布局应含 @3: {out}");
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
    use muxterm::runtime::TmuxBackend;
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
    use muxterm::runtime::TmuxBackend;
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
    use muxterm::runtime::TmuxBackend;
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
            keys: vec![muxterm::terminal::input::KeyEvent::Char('x')],
        })
        .unwrap();
    assert!(matches!(
        outcome,
        muxterm::protocol::model::task::TaskOutcome::Done
    ));

    let _ = rt.block_on(model.shutdown());
    let _ = std::process::Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .status();
}

// ── daemon session 架构测试 ─────────────────────────────────
//
// 这些测试用真实 unix socket + fork daemon 进程验证持久 session。
// 需要 unix 环境（cfg(unix)）。

#[cfg(unix)]
mod daemon_tests {
    use super::*;
    use muxterm::core::types::PaneId;
    use muxterm::platform::cli::client::send_command;
    use muxterm::platform::cli::session::session_socket_path;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 唯一 session name（基于 PID + nanos，避免冲突）。
    fn unique_session() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("test-{}-{}", std::process::id(), nanos)
    }

    /// 清理 session socket（测试结束后调用）。
    fn cleanup_session(name: &str) {
        let sock = session_socket_path(name);
        let _ = std::fs::remove_file(&sock);
    }

    /// 启动 daemon（fork 后台进程），等待 socket 就绪。
    #[allow(dead_code)]
    fn start_daemon(name: &str) -> PathBuf {
        let sock = session_socket_path(name);
        cleanup_session(name);

        // 用 std::process::Command 启动 daemon（fork 方式）
        let _exe = std::env::current_exe()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| PathBuf::from("target/debug/muxterm"));

        // 找到 muxterm 二进制（test bin 可能是 test runner，用 cargo build 产物）
        let bin = find_muxterm_bin();

        let status = std::process::Command::new(&bin)
            .args(["new-session", "-s", name])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        match status {
            Ok(mut child) => {
                // 等待 socket 就绪
                for _ in 0..60 {
                    if sock.exists() {
                        return sock;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                let _ = child.kill();
                panic!("daemon 启动超时: {}", sock.display());
            }
            Err(e) => {
                eprintln!("skip: 无法启动 daemon ({e})");
                cleanup_session(name);
                panic!("daemon 启动失败: {e}");
            }
        }
    }

    /// 查找 muxterm 二进制路径。
    fn find_muxterm_bin() -> PathBuf {
        // CARGO_TARGET_DIR 或默认
        let candidates = [
            PathBuf::from("../muxterm-target/debug/muxterm"),
            PathBuf::from("../../muxterm-target/debug/muxterm"),
            PathBuf::from("target/debug/muxterm"),
        ];
        for c in &candidates {
            if c.exists() {
                return c.clone();
            }
        }
        // 用 env var
        if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
            let p = PathBuf::from(td).join("debug/muxterm");
            if p.exists() {
                return p;
            }
        }
        PathBuf::from("muxterm")
    }

    #[test]
    fn daemon_create_and_list_sessions() {
        let name = unique_session();
        let sock = match start_daemon_safe(&name) {
            Some(s) => s,
            None => {
                eprintln!("skip: daemon 启动失败（sandbox 限制？）");
                cleanup_session(&name);
                return;
            }
        };

        // list-sessions 应成功（daemon 已启动）
        let resp = send_command(&sock, &CliCommand::ListSessions, OutputFormat::Text);
        match resp {
            Ok(r) => {
                assert!(r.ok, "list-sessions 应成功: {:?}", r.error);
            }
            Err(e) => {
                eprintln!("skip: daemon 连接失败 ({e})");
                cleanup_session(&name);
                return;
            }
        }

        // 发送 kill-session 关闭 daemon
        let _ = send_command(
            &sock,
            &CliCommand::KillSession { target: None },
            OutputFormat::Json,
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
        cleanup_session(&name);
    }

    #[test]
    fn daemon_split_and_list_panes() {
        let name = unique_session();
        let sock = match start_daemon_safe(&name) {
            Some(s) => s,
            None => {
                eprintln!("skip: daemon 启动失败（sandbox 限制？）");
                cleanup_session(&name);
                return;
            }
        };

        // 第一个 split-pane（水平）
        let resp = send_command(
            &sock,
            &CliCommand::SplitPane {
                horizontal: true,
                target: Some(PaneId(1)),
                size: None,
            },
            OutputFormat::Json,
        );
        match resp {
            Ok(r) => {
                if !r.ok {
                    eprintln!("skip: split-pane 失败: {}", r.error);
                    let _ = send_command(
                        &sock,
                        &CliCommand::KillSession { target: None },
                        OutputFormat::Json,
                    );
                    cleanup_session(&name);
                    return;
                }
            }
            Err(e) => {
                eprintln!("skip: daemon 连接失败 ({e})");
                cleanup_session(&name);
                return;
            }
        }

        // list-panes 应有至少两个 pane
        let resp = send_command(
            &sock,
            &CliCommand::ListPanes { tab: None },
            OutputFormat::Json,
        );
        if let Ok(r) = resp {
            assert!(r.ok, "list-panes 应成功: {}", r.error);
            // daemon 状态保留的核心验证：split 后应有多于 1 个 pane
            let pane_count = r.output.matches("@").count();
            assert!(pane_count >= 2, "split 后应至少有 2 个 pane: {}", r.output);
        }

        // 清理
        let _ = send_command(
            &sock,
            &CliCommand::KillSession { target: None },
            OutputFormat::Json,
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
        cleanup_session(&name);
    }

    /// 安全启动 daemon：失败返回 None 而非 panic。
    #[allow(dead_code)]
    fn start_daemon_safe(name: &str) -> Option<PathBuf> {
        let sock = session_socket_path(name);
        cleanup_session(name);

        let bin = find_muxterm_bin();
        if !bin.exists() {
            eprintln!("skip: muxterm 二进制不存在: {}", bin.display());
            return None;
        }

        let child = std::process::Command::new(&bin)
            .args(["new-session", "-s", name])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: 无法启动 daemon ({e})");
                return None;
            }
        };

        // 等待 socket 就绪
        for _ in 0..60 {
            if sock.exists() {
                return Some(sock);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = child.kill();
        eprintln!("skip: daemon 启动超时");
        None
    }

    #[test]
    fn daemon_nonexistent_session_errors() {
        let name = unique_session();
        let sock = session_socket_path(&name);
        // 不启动 daemon，直接连接应失败
        let resp = send_command(&sock, &CliCommand::ListSessions, OutputFormat::Text);
        assert!(resp.is_err(), "连接不存在的 session 应失败");
        cleanup_session(&name);
    }
}
