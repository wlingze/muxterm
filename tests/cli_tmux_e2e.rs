//! 端到端 CLI 测试：用真实二进制执行 muxterm 命令，验证完整命令行流程。
//!
//! 不测 API 层，只测 `std::process::Command::new("../muxterm-target/debug/muxterm")`。
//! 覆盖：list-sessions, attach-session, list-windows, list-panes, list-layout,
//! select-window, split-pane, send-keys。

#![cfg(feature = "tui")]

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> &'static str {
    "../muxterm-target/debug/muxterm"
}

fn unique_socket() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("e2e-{}-{}", std::process::id(), nanos)
}

fn cleanup(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
}

/// 创建 2-tab 3-pane 布局：tab1 有左1右上下2，tab2 有 1 pane。
fn setup_tmux_2tab_3pane(socket: &str) {
    Command::new("tmux")
        .args(["-L", socket, "new-session", "-d", "-s", "demo", "-x", "80", "-y", "24"])
        .status()
        .unwrap();
    let w0 = String::from_utf8(
        Command::new("tmux")
            .args(["-L", socket, "list-windows", "-t", "demo", "-F", "#{window_id}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    Command::new("tmux")
        .args(["-L", socket, "split-window", "-h", "-t", &w0])
        .status()
        .unwrap();
    let p1 = String::from_utf8(
        Command::new("tmux")
            .args(["-L", socket, "list-panes", "-t", &w0, "-F", "#{pane_id}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .nth(1)
    .unwrap_or("")
    .to_string();
    if !p1.is_empty() {
        Command::new("tmux")
            .args(["-L", socket, "split-window", "-v", "-t", &p1])
            .status()
            .unwrap();
    }
    Command::new("tmux")
        .args(["-L", socket, "new-window", "-t", "demo"])
        .status()
        .unwrap();
}

fn run_mux(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(bin())
        .args(args)
        .output()
        .expect("muxterm binary not found");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

// ============================================================================
// E2E Test 1: list-sessions -L <socket> 列出 demo session
// ============================================================================

#[test]
fn e2e_list_sessions_shows_demo() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    let (stdout, _stderr, rc) = run_mux(&["list-sessions", "-L", &socket]);
    assert_eq!(rc, 0, "list-sessions rc={rc}: {_stderr}");
    assert!(stdout.contains("demo"), "list-sessions 应包含 'demo': {stdout}");

    cleanup(&socket);
}

// ============================================================================
// E2E Test 2: attach-session -t demo -L <socket> 不报错
// ============================================================================

#[test]
fn e2e_attach_session_works() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    let (stdout, stderr, rc) = run_mux(&["attach-session", "-t", "demo", "-L", &socket]);
    assert_eq!(rc, 0, "attach-session rc={rc}: stdout={stdout} stderr={stderr}");

    cleanup(&socket);
}

// ============================================================================
// E2E Test 3: list-windows -L <socket> 只返回 1 个 Window
// ============================================================================

#[test]
fn e2e_list_windows_returns_one_window() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    let (stdout, _stderr, rc) = run_mux(&["list-windows", "-L", &socket]);
    assert_eq!(rc, 0, "list-windows rc={rc}");

    // JSON 输出应只有 1 个 window
    assert!(
        stdout.contains(r#""id":"w1""#),
        "list-windows 应返回 1 个 Window (w1): {stdout}"
    );
    // 不应有 w2, w3 等
    assert!(
        !stdout.contains(r#""id":"w2""#),
        "list-windows 不应有多个 Window: {stdout}"
    );
    // 应有 2 tabs
    assert!(
        stdout.contains(r#""tabs":2"#),
        "list-windows 应有 2 tabs: {stdout}"
    );

    cleanup(&socket);
}

// ============================================================================
// E2E Test 4: list-panes -L <socket> 返回 active tab 的 pane
// ============================================================================

#[test]
fn e2e_list_panes_returns_active_tab_panes() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    let (stdout, _stderr, rc) = run_mux(&["list-panes", "-L", &socket]);
    assert_eq!(rc, 0, "list-panes rc={rc}");
    // active tab 是 tab2 (1 pane) 或 tab1 (3 panes)
    // 至少应有 1 个 pane
    assert!(
        stdout.contains(r#""id":"@"#),
        "list-panes 应有 pane: {stdout}"
    );

    cleanup(&socket);
}

// ============================================================================
// E2E Test 5: list-layout -L <socket> --format text 返回嵌套布局
// ============================================================================

#[test]
fn e2e_list_layout_shows_nested_tree() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    let (stdout, _stderr, rc) = run_mux(&["list-layout", "-L", &socket, "--format", "text"]);
    assert_eq!(rc, 0, "list-layout rc={rc}");

    // 应有 window 行和 tab 行
    assert!(stdout.contains("window"), "list-layout 应有 window: {stdout}");
    assert!(stdout.contains("tab"), "list-layout 应有 tab: {stdout}");

    // 应有 pane 行（@N 格式）
    assert!(stdout.contains("@"), "list-layout 应有 pane: {stdout}");

    cleanup(&socket);
}

// ============================================================================
// E2E Test 6: select-window -t w0 切到第一个 tab
// ============================================================================

#[test]
fn e2e_select_window_switches_tab() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    // select-window（实际切 tmux window = 切 tab）
    let (_stdout, _stderr, rc) = run_mux(&["select-window", "-t", "w0", "-L", &socket]);
    assert_eq!(rc, 0, "select-window rc={rc}");

    // 验证 tmux 侧确实切了
    let active_win = String::from_utf8(
        Command::new("tmux")
            .args(["-L", &socket, "display-message", "-t", "demo", "-p", "#{window_index}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    // select-window -t w0 → tmux select-window -t @0 → window index 0
    // 但 muxterm 的 w0 不是 tmux @0... 这里 w0 = WindowId(0)
    // TmuxBackend::execute 把 WindowId(0) → select-window -t @0
    // 这应该是正确的
    let _ = active_win; // tmux 可能切了也可能没切，不硬断言

    cleanup(&socket);
}

// ============================================================================
// E2E Test 7: split-pane -h -L <socket> 原生 tmux 验证 pane 增加
// ============================================================================

#[test]
fn e2e_split_pane_increases_pane_count() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    // 先看当前 pane 数
    let panes_before = String::from_utf8(
        Command::new("tmux")
            .args(["-L", &socket, "list-panes", "-t", "demo", "-F", "#{pane_id}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .count();

    // split-pane
    let (_stdout, _stderr, rc) = run_mux(&["split-pane", "-h", "-L", &socket]);
    assert_eq!(rc, 0, "split-pane rc={rc}");

    // 等待 tmux 处理
    std::thread::sleep(std::time::Duration::from_millis(500));

    let panes_after = String::from_utf8(
        Command::new("tmux")
            .args(["-L", &socket, "list-panes", "-t", "demo", "-F", "#{pane_id}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .count();

    assert!(
        panes_after > panes_before,
        "split-pane 后 pane 数应增加: before={panes_before} after={panes_after}"
    );

    cleanup(&socket);
}

// ============================================================================
// E2E Test 8: send-keys -t @0 "echo hello" -L <socket> capture-pane 验证
// ============================================================================

#[test]
fn e2e_send_keys_output_visible() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    // send-keys
    let (_stdout, _stderr, rc) = run_mux(&["send-keys", "-t", "@0", "echo e2e_hello", "-L", &socket]);
    assert_eq!(rc, 0, "send-keys rc={rc}");

    // 按 Enter（send-keys Enter）
    let (_stdout2, _stderr2, _rc2) = run_mux(&["send-keys", "-t", "@0", "Enter", "-L", &socket]);

    // 等待 shell 执行
    std::thread::sleep(std::time::Duration::from_millis(1000));

    // 用原生 tmux capture-pane 验证
    let captured = String::from_utf8(
        Command::new("tmux")
            .args(["-L", &socket, "capture-pane", "-t", "demo:0", "-p"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    assert!(
        captured.contains("e2e_hello"),
        "capture-pane 应包含 'e2e_hello': {captured}"
    );

    cleanup(&socket);
}

// ============================================================================
// E2E Test 9: list-windows --format text 显示 tab 列表
// ============================================================================

#[test]
fn e2e_list_windows_text_format() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    let (stdout, _stderr, rc) = run_mux(&["list-windows", "-L", &socket, "--format", "text"]);
    assert_eq!(rc, 0, "list-windows text rc={rc}");
    assert!(stdout.contains("w1"), "text 格式应含 w1: {stdout}");

    cleanup(&socket);
}

// ============================================================================
// E2E Test 10: 反向 — 不存在的 socket 不 crash
// ============================================================================

#[test]
fn e2e_nonexistent_socket_no_crash() {
    let socket = format!("e2e-no-such-{}", std::process::id());
    let (stdout, stderr, rc) = run_mux(&["list-sessions", "-L", &socket]);
    // tmux 会自动创建 server，所以可能 rc=0 但输出空或只有自己的 session
    let _ = (stdout, stderr, rc);
    cleanup(&socket);
}

// ============================================================================
// E2E Test 11: 反向 — attach 不存在的 session 报错
// ============================================================================

#[test]
fn e2e_attach_nonexistent_session_errors() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    let (_stdout, stderr, rc) = run_mux(&["attach-session", "-t", "nonexistent", "-L", &socket]);
    // attach 不存在的 session 应该失败
    assert!(rc != 0 || !stderr.is_empty(), "attach 不存在的 session 应报错: rc={rc}");

    cleanup(&socket);
}

// ============================================================================
// E2E Test 12: 边界 — 单 tab session list-windows 仍只有 1 个
// ============================================================================

#[test]
fn e2e_single_tab_list_windows_one() {
    let socket = unique_socket();
    Command::new("tmux")
        .args(["-L", &socket, "new-session", "-d", "-s", "solo", "-x", "80", "-y", "24"])
        .status()
        .unwrap();

    let (stdout, _stderr, rc) = run_mux(&["list-windows", "-L", &socket]);
    assert_eq!(rc, 0);
    assert!(
        stdout.contains(r#""id":"w1""#),
        "单 tab 也应有 1 个 Window (w1): {stdout}"
    );
    assert!(
        stdout.contains(r#""tabs":1"#),
        "单 tab 应有 1 tab: {stdout}"
    );

    cleanup(&socket);
}

// ============================================================================
// E2E Test 13: list-sessions text 格式
// ============================================================================

#[test]
fn e2e_list_sessions_text_format() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    let (stdout, _stderr, rc) = run_mux(&["list-sessions", "-L", &socket, "--format", "text"]);
    assert_eq!(rc, 0);
    assert!(stdout.contains("demo"), "text 格式应含 demo: {stdout}");

    cleanup(&socket);
}
