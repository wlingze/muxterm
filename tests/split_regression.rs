//! 分割回归测试：逐层验证 muxterm CLI tmux pane split 真实行为。
//!
//! 硬约束：硬超时 + 独立 tmux socket + 清理。不用 sleep 作为修复。

#![cfg(feature = "ffi")]

use std::process::Command;
use std::time::Duration;

fn muxterm_bin() -> std::path::PathBuf {
    let target =
        std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "../muxterm-target".to_string());
    let p = std::path::PathBuf::from(&target)
        .join("debug")
        .join("muxterm");
    if p.exists() {
        return p;
    }
    std::path::PathBuf::from("target/debug/muxterm")
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos().to_string())
        .unwrap_or_else(|_| "0".into())
}

fn unique_socket(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("muxterm-split-{}-{}", label, nanos)
}

fn create_session(socket: &str, name: &str) {
    let output = Command::new("tmux")
        .args([
            "-L",
            socket,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            name,
            "-x",
            "80",
            "-y",
            "24",
        ])
        .output()
        .expect("创建 tmux session 失败");
    assert!(
        output.status.success(),
        "tmux new-session 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn count_panes(socket: &str, session: &str) -> usize {
    let output = Command::new("tmux")
        .args([
            "-L",
            socket,
            "list-panes",
            "-t",
            session,
            "-F",
            "#{pane_id}",
        ])
        .output()
        .expect("list-panes 失败");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .count()
}

fn session_exists_on_default(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", name])
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn kill_server(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
}

fn kill_default_session(name: &str) {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", name])
        .output();
}

/// ── Layer 0: CLI --socket propagation ──
/// 证明 muxterm CLI tmux 命令能正确定位到指定 tmux socket。
#[test]
fn cli_socket_propagation_targets_correct_tmux_server() {
    let socket = unique_socket("layer0");
    let session = format!("sock-test-{}", rand_suffix());
    create_session(&socket, &session);
    std::thread::sleep(Duration::from_millis(300));

    // 确认 session 在独立 socket 上存在
    let on_isolated = Command::new("tmux")
        .args(["-L", &socket, "has-session", "-t", &session])
        .output()
        .unwrap();
    assert!(
        on_isolated.status.success(),
        "session 应在独立 socket 上存在"
    );

    // 确认 session 不在默认 socket 上
    assert!(
        !session_exists_on_default(&session),
        "session 不应在默认 socket 上存在"
    );

    // 通过 muxterm CLI 用 --socket 指定独立 socket，列 tab
    let bin = muxterm_bin();
    let output = Command::new(&bin)
        .args([
            "tmux",
            "tab",
            "list",
            "--target",
            "local",
            "--socket",
            &socket,
            "--session",
            &session,
        ])
        .output()
        .expect("muxterm 执行失败");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"ok\":true"),
        "tab list 用 --socket 应返回 ok: {stdout}"
    );
    assert!(stdout.contains("\"tabs\""), "应返回 tabs 数据: {stdout}");

    // 清理：验证没有泄漏到默认 socket
    assert!(
        !session_exists_on_default(&session),
        "CLI 不应在默认 socket 上创建 session"
    );
    kill_server(&socket);
}

/// ── Layer 0b: CLI --socket 不泄漏 session 到默认 socket ──
#[test]
fn cli_socket_does_not_leak_to_default_server() {
    let socket = unique_socket("layer0b");
    let session = format!("leak-test-{}", rand_suffix());
    create_session(&socket, &session);
    std::thread::sleep(Duration::from_millis(300));

    let bin = muxterm_bin();

    // 执行 pane list（只读操作）
    let _ = Command::new(&bin)
        .args([
            "tmux",
            "pane",
            "list",
            "--target",
            "local",
            "--socket",
            &socket,
            "--session",
            &session,
        ])
        .output();

    // 确认 session 没有泄漏到默认 socket
    assert!(
        !session_exists_on_default(&session),
        "pane list 不应在默认 socket 创建 session"
    );

    // 执行 tab list
    let _ = Command::new(&bin)
        .args([
            "tmux",
            "tab",
            "list",
            "--target",
            "local",
            "--socket",
            &socket,
            "--session",
            &session,
        ])
        .output();

    assert!(
        !session_exists_on_default(&session),
        "tab list 不应在默认 socket 创建 session"
    );

    kill_server(&socket);
}

/// ── Layer 1: 真实 binary split ──
/// 创建独立 socket session → 通过 muxterm binary 用 --socket 执行 split → 用原生 tmux 验证 pane 数增加。
///
/// ROOT CAUSE: TmuxBackend::dispatch_command 用 try_send 到异步 channel，
/// sender task 在 tokio runtime 中异步写 pty。CLI exec 在 execute 后立即 shutdown，
/// shutdown drop cmd_tx → sender task 结束 → 命令可能未到达 tmux。
/// 修复方案：CLI exec 在 execute 后等命令实际完成（通过 refresh/poll 等待 layout-change 事件）。
#[test]
fn split_real_binary_increases_pane_count() {
    let socket = unique_socket("layer1");
    let session = format!("split-test-{}", rand_suffix());
    create_session(&socket, &session);

    assert_eq!(count_panes(&socket, &session), 1, "初始应有 1 pane");

    let bin = muxterm_bin();
    assert!(bin.exists(), "muxterm binary 不存在");

    let output = Command::new(&bin)
        .args([
            "tmux",
            "pane",
            "split",
            "--target",
            "local",
            "--socket",
            &socket,
            "--session",
            &session,
            "--pane",
            "1",
            "--direction",
            "horizontal",
        ])
        .output()
        .expect("muxterm split 执行失败");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("\"ok\":true"),
        "split 应返回 ok: stdout={stdout} stderr={stderr}"
    );

    // 用原生 tmux 验证 pane 数量增加（非 sleep 修复：等待事件回流）
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut panes_after = 0;
    while std::time::Instant::now() < deadline {
        panes_after = count_panes(&socket, &session);
        if panes_after >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        panes_after, 2,
        "split 后原生 tmux 应有 2 panes，实际 {panes_after}\nstdout={stdout}\nstderr={stderr}"
    );

    // 验证没有泄漏到默认 socket
    assert!(
        !session_exists_on_default(&session),
        "split 不应在默认 socket 创建 session"
    );

    kill_server(&socket);
}

/// ── Layer 2: CLI parse → Task::SplitPane 映射 ──
#[test]
fn split_cli_parse_produces_correct_command() {
    use muxterm::platform::cli::tmux_cli::{
        parse_tmux_cli, PaneCmd, SplitDirection, Target, TmuxCliCommand,
    };

    let args: Vec<String> = [
        "pane",
        "split",
        "--target",
        "local",
        "--socket",
        "my-socket",
        "--session",
        "test-session",
        "--pane",
        "1",
        "--direction",
        "horizontal",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let cmd = parse_tmux_cli(&args).expect("parse 失败");
    match cmd {
        TmuxCliCommand::Pane(PaneCmd::Split {
            target,
            socket,
            session,
            pane,
            direction,
        }) => {
            assert_eq!(target, Target::Local);
            assert_eq!(socket.as_deref(), Some("my-socket"));
            assert_eq!(session, "test-session");
            assert_eq!(pane, 1);
            assert_eq!(direction, SplitDirection::Horizontal);
        }
        other => panic!("解析结果不是 PaneSplit: {other:?}"),
    }
}

/// ── Layer 3: command builder split-window 正确 target ──
#[test]
fn split_window_command_uses_correct_target() {
    use muxterm::core::tmux::command::{split_window, SplitDirection, WindowId};

    // WindowId(0) → @0 in tmux (first window)
    let cmd = split_window(WindowId(0), SplitDirection::Horizontal, None);
    let line = cmd.to_line();
    assert!(
        line.contains("split-window"),
        "命令应含 split-window: {line}"
    );
    assert!(line.contains("-h"), "水平分割应含 -h: {line}");
    assert!(line.contains("@0"), "target 应含 tmux window id @0: {line}");
}

/// ── Layer 3b: PaneId → TabId → WindowId 映射 ──
/// 验证 backend 从 pane 查找 tab_id 再转 WindowId 的映射链正确。
/// 用真实 tmux 验证 #{window_id} 是 @N 格式（不是 window_index）。
#[test]
fn paneid_to_tabid_to_windowid_mapping_matches_tmux_window_id() {
    use muxterm::core::tmux::command::{split_window, SplitDirection, WindowId};

    let socket = unique_socket("layer3b");
    let session = format!("map-test-{}", rand_suffix());
    create_session(&socket, &session);
    std::thread::sleep(Duration::from_millis(300));

    // 从真实 tmux 获取 window_id
    let output = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "list-windows",
            "-t",
            &session,
            "-F",
            "#{window_id}",
        ])
        .output()
        .unwrap();
    let tmux_window_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        tmux_window_id.starts_with('@'),
        "tmux window_id 应以 @ 开头: {tmux_window_id}"
    );
    let win_num: u32 = tmux_window_id[1..].parse().expect("window_id 数字");

    // 验证 split_window(WindowId(win_num), ...) 生成的命令 target 是正确的 @N
    let cmd = split_window(WindowId(win_num), SplitDirection::Horizontal, None);
    let line = cmd.to_line();
    assert!(
        line.contains(&tmux_window_id),
        "split-window 命令应含真实 tmux window_id {tmux_window_id}: {line}"
    );

    // 验证该 target 在真实 tmux 上能执行 split
    let output = Command::new("tmux")
        .args(["-L", &socket, "split-window", "-h", "-t", &tmux_window_id])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tmux split-window -t {tmux_window_id} 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let panes = count_panes(&socket, &session);
    assert_eq!(panes, 2, "split 后应有 2 panes");

    kill_server(&socket);
}

/// ── Layer 4: backend split-window 命令实际发送到 tmux ──
/// 这个测试保持 model 存活足够长，所以命令能到达 tmux。
#[test]
fn backend_split_actually_creates_pane_in_tmux() {
    use muxterm::core::model::layout::SplitDir;
    use muxterm::core::model::task::Task;
    use muxterm::core::model::TerminalModel;
    use muxterm::core::runtime::TmuxBackend;

    let socket = unique_socket("layer4");
    let session = format!("backend-split-{}", rand_suffix());

    Command::new("tmux")
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
        ])
        .output()
        .expect("创建 tmux session 失败");
    std::thread::sleep(Duration::from_millis(300));

    let backend = TmuxBackend::new_with_attach(Some(&socket), &session);
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("build runtime");

    rt.block_on(model.connect()).expect("connect 失败");
    let _ = model.poll_events();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let _ = model.refresh();
        if model.state().active_pane().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let active_pane = model.state().active_pane().expect("应有 active pane");
    let pane_id = active_pane.id;

    let result = model.execute(Task::SplitPane {
        target: Some(pane_id),
        dir: SplitDir::Horizontal,
        command: None,
        workdir: None,
    });
    assert!(result.is_ok(), "SplitPane 执行失败: {:?}", result.err());

    // 等待 layout-change 事件回流（不是 sleep，而是事件驱动等待）
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let _ = model.refresh();
        if let Some(tab) = model.state().active_tab() {
            let panes = model.state().panes(&tab.id);
            if panes.len() >= 2 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let panes_after = count_panes(&socket, &session);
    assert_eq!(
        panes_after, 2,
        "backend split 后原生 tmux 应有 2 panes，实际 {panes_after}"
    );

    let _ = rt.block_on(model.shutdown());
    kill_server(&socket);
}

/// ── Layer 5: CLI exec 命令完成前等待事件回流 ──
/// 验证 CLI exec 在 execute SplitPane 后、shutdown 前，等待 layout-change 事件。
/// 这个测试证明根因修复：CLI 不在命令未到达 tmux 时就退出。
#[test]
fn cli_exec_waits_for_command_completion_before_shutdown() {
    let socket = unique_socket("layer5");
    let session = format!("wait-test-{}", rand_suffix());
    create_session(&socket, &session);
    std::thread::sleep(Duration::from_millis(300));

    let bin = muxterm_bin();
    assert!(bin.exists());

    // 执行 split（CLI 应等命令到达 tmux 后才退出）
    let output = Command::new(&bin)
        .args([
            "tmux",
            "pane",
            "split",
            "--target",
            "local",
            "--socket",
            &socket,
            "--session",
            &session,
            "--pane",
            "1",
            "--direction",
            "horizontal",
        ])
        .output()
        .expect("muxterm split 执行失败");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"ok\":true"), "split 应返回 ok: {stdout}");

    // CLI 退出后立即检查（不应需要额外等待）
    let panes_after = count_panes(&socket, &session);
    assert_eq!(
        panes_after, 2,
        "CLI 退出后应立即有 2 panes（命令已完成）: {panes_after}\nstdout={stdout}"
    );

    // 验证没有泄漏
    assert!(
        !session_exists_on_default(&session),
        "不应泄漏到默认 socket"
    );

    kill_server(&socket);
}

// ═══════════════════════════════════════════════════════════════
// Layer 5: deep nested splits (3 levels: H → V → H) via CLI binary
// ═══════════════════════════════════════════════════════════════

#[test]
fn deep_nested_splits_three_levels() {
    let socket = unique_socket("deep3");
    let session = format!("deep-{}", rand_suffix());

    // 创建 detached session
    Command::new("tmux")
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
            "120",
            "-y",
            "40",
        ])
        .status()
        .expect("tmux new-session 失败");

    // 3 次 split via muxterm tmux CLI (returns JSON envelope)
    for (i, dir) in ["horizontal", "vertical", "horizontal"].iter().enumerate() {
        let output = Command::new(muxterm_bin())
            .args([
                "tmux",
                "pane",
                "split",
                "--socket",
                &socket,
                "--session",
                &session,
                "--pane",
                "1",
                "--direction",
                dir,
            ])
            .output()
            .expect("muxterm split 执行失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("\"ok\":true"),
            "split {} 应返回 ok: {stdout}",
            i + 1
        );
    }

    // 验证 pane 数 >= 4（用原生 tmux 验证）
    let panes = count_panes(&socket, &session);
    assert!(panes >= 4, "3 次 split 后应有 >= 4 pane: got {panes}");

    // 验证 layout：用 muxterm tmux CLI 查询 pane list，验证有 >= 4 pane
    let output = Command::new(muxterm_bin())
        .args([
            "tmux",
            "pane",
            "list",
            "--socket",
            &socket,
            "--session",
            &session,
        ])
        .output()
        .expect("pane list 失败");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"ok\":true"),
        "pane list 应返回 ok: {stdout}"
    );
    // 统计 pane 数量（JSON 中 "id" 字段出现次数）
    let pane_count = stdout.matches("\"id\"").count();
    assert!(
        pane_count >= 4,
        "pane list 应有 >= 4 pane: got {pane_count} stdout={stdout}"
    );

    // 验证没有泄漏到默认 socket
    assert!(
        !session_exists_on_default(&session),
        "不应泄漏到默认 socket"
    );

    kill_server(&socket);
}
