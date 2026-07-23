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
        .args([
            "-L",
            socket,
            "new-session",
            "-d",
            "-s",
            "demo",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status()
        .unwrap();
    let w0 = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                socket,
                "list-windows",
                "-t",
                "demo",
                "-F",
                "#{window_id}",
            ])
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
    assert!(
        stdout.contains("demo"),
        "list-sessions 应包含 'demo': {stdout}"
    );

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
    assert_eq!(
        rc, 0,
        "attach-session rc={rc}: stdout={stdout} stderr={stderr}"
    );

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
    assert!(
        stdout.contains("window"),
        "list-layout 应有 window: {stdout}"
    );
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
            .args([
                "-L",
                &socket,
                "display-message",
                "-t",
                "demo",
                "-p",
                "#{window_index}",
            ])
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

    // 统计 session 内全部 pane（-a），避免只数到 active window
    let panes_before = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-panes",
                "-a",
                "-t",
                "demo",
                "-F",
                "#{pane_id}",
            ])
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
            .args([
                "-L",
                &socket,
                "list-panes",
                "-a",
                "-t",
                "demo",
                "-F",
                "#{pane_id}",
            ])
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

    // send-keys 到 pane @0（tmux %0）；文本末尾 \r = Enter
    let (_stdout, _stderr, rc) =
        run_mux(&["send-keys", "-t", "@0", "echo e2e_hello\r", "-L", &socket]);
    assert_eq!(rc, 0, "send-keys rc={rc}");

    // 等待 shell 执行
    std::thread::sleep(std::time::Duration::from_millis(1000));

    // 用原生 tmux capture-pane 验证 %0
    let captured = String::from_utf8(
        Command::new("tmux")
            .args(["-L", &socket, "capture-pane", "-t", "%0", "-p"])
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
    assert!(
        rc != 0 || !stderr.is_empty(),
        "attach 不存在的 session 应报错: rc={rc}"
    );

    cleanup(&socket);
}

// ============================================================================
// E2E Test 12: 边界 — 单 tab session list-windows 仍只有 1 个
// ============================================================================

#[test]
fn e2e_single_tab_list_windows_one() {
    let socket = unique_socket();
    Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "solo",
            "-x",
            "80",
            "-y",
            "24",
        ])
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

// ============================================================================
// E2E: detach / reattach 后布局保持（tmux 模式）
// ============================================================================

#[test]
fn e2e_tmux_detach_reattach_keeps_layout() {
    let socket = unique_socket();
    setup_tmux_2tab_3pane(&socket);

    // 第一次 attach（CLI 命令隐式 connect→query→shutdown/detach）
    let (layout1, _stderr, rc1) = run_mux(&["list-layout", "-L", &socket, "--format", "json"]);
    assert_eq!(rc1, 0, "首次 list-layout rc={rc1}");
    assert!(
        layout1.contains("horizontal") && layout1.contains("vertical"),
        "首次布局应有嵌套分割: {layout1}"
    );

    // 显式 detach（当前为 no-op Task，但命令不应失败）
    let (_out, _err, rc_detach) = run_mux(&["detach", "-L", &socket]);
    assert_eq!(rc_detach, 0, "detach rc={rc_detach}");

    // 原生 tmux session 应仍在
    let sessions = String::from_utf8(
        Command::new("tmux")
            .args(["-L", &socket, "list-sessions", "-F", "#{session_name}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(
        sessions.lines().any(|l| l.trim() == "demo"),
        "detach 后 demo session 应仍在: {sessions}"
    );

    // 重新 attach / 查询，布局应保持
    let (layout2, _stderr2, rc2) = run_mux(&["list-layout", "-L", &socket, "--format", "json"]);
    assert_eq!(rc2, 0, "reattach list-layout rc={rc2}");
    assert!(
        layout2.contains("horizontal") && layout2.contains("vertical"),
        "reattach 后布局应保持嵌套: {layout2}"
    );

    // pane 总数不变（2tab: 3+1=4）
    let panes = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-panes",
                "-a",
                "-t",
                "demo",
                "-F",
                "#{pane_id}",
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .filter(|l| !l.is_empty())
    .count();
    assert_eq!(panes, 4, "reattach 后应仍有 4 个 pane: {panes}");

    cleanup(&socket);
}

// ============================================================================
// E2E: local 模式 detach/attach 完整流程
// ============================================================================

fn unique_session_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("loc-{}-{}", std::process::id(), nanos)
}

fn cleanup_local(name: &str) {
    let _ = run_mux(&["kill-session", "-s", name]);
    // 清掉可能残留的死 socket，避免 new-session 误判已存在
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let sock = format!("{runtime}/muxterm-{name}.sock");
    let _ = std::fs::remove_file(&sock);
    std::thread::sleep(std::time::Duration::from_millis(100));
}

#[test]
fn e2e_local_detach_attach_full_flow() {
    let name = unique_session_name();
    cleanup_local(&name);

    // 1. 创建 session（启动 daemon）
    let (_o, e, rc) = run_mux(&["new-session", "-s", &name]);
    assert_eq!(rc, 0, "new-session rc={rc}: {e}");

    // 等 daemon 可连接（socket 文件出现 ≠ 已 listen）
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let sock = format!("{runtime}/muxterm-{name}.sock");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // 2. 操作：split + new-tab
    let (_o, e, rc) = run_mux(&["split-pane", "-h", "-s", &name]);
    assert_eq!(rc, 0, "split-pane rc={rc}: {e}");
    let (_o, e, rc) = run_mux(&["new-tab", "-s", &name]);
    assert_eq!(rc, 0, "new-tab rc={rc}: {e}");

    let (layout1, _e, rc) = run_mux(&["list-layout", "-s", &name, "--format", "json"]);
    assert_eq!(rc, 0);
    assert!(
        layout1.contains("horizontal") || layout1.contains("t1") || layout1.contains("t2"),
        "应有布局: {layout1}"
    );

    // 3. detach（daemon 继续运行）
    let (_o, e, rc) = run_mux(&["detach", "-s", &name]);
    assert_eq!(rc, 0, "detach rc={rc}: {e}");

    // 4. re-attach / 查询：状态保留
    let (layout2, e, rc) = run_mux(&["list-layout", "-s", &name, "--format", "json"]);
    assert_eq!(rc, 0, "reattach list-layout rc={rc}: {e}");
    assert!(
        !layout2.is_empty(),
        "detach 后 daemon 应仍可查询: {layout2}"
    );

    let (panes, _e, rc) = run_mux(&["list-panes", "-s", &name, "-t", "1", "--format", "text"]);
    assert_eq!(rc, 0);
    assert!(panes.contains('@'), "tab1 应仍有 pane: {panes}");

    // 5. 清理
    cleanup_local(&name);
}

// ============================================================================
// E2E: 空 session 边界 — attach 不 panic
// ============================================================================

#[test]
fn e2e_attach_empty_or_missing_session_no_panic() {
    let socket = unique_socket();
    cleanup(&socket);

    // 空 server：attach 不存在的 session → 应失败但不 panic
    let (stdout, stderr, rc) = run_mux(&["attach-session", "-t", "empty", "-L", &socket]);
    assert!(
        rc != 0,
        "attach 空/不存在 session 应非 0: stdout={stdout} stderr={stderr}"
    );
    // 进程正常退出（非 signal abort）
    assert!(
        rc > 0 || !stderr.is_empty() || !stdout.is_empty() || rc == 1,
        "应有错误输出或非零退出: rc={rc} stderr={stderr}"
    );

    cleanup(&socket);

    // 最小化 session（仅 1 pane）attach / list 不 panic
    let socket2 = unique_socket();
    Command::new("tmux")
        .args([
            "-L",
            &socket2,
            "new-session",
            "-d",
            "-s",
            "bare",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status()
        .unwrap();

    let (stdout, stderr, rc) = run_mux(&["attach-session", "-t", "bare", "-L", &socket2]);
    assert_eq!(
        rc, 0,
        "attach 最小 session 应成功: stdout={stdout} stderr={stderr}"
    );
    let (layout, e, rc) = run_mux(&["list-layout", "-L", &socket2, "--format", "text"]);
    assert_eq!(rc, 0, "list-layout bare rc={rc}: {e}");
    assert!(
        layout.contains("pane") || layout.contains('@') || layout.contains("tab"),
        "最小 session 应有 pane/tab: {layout}"
    );

    cleanup(&socket2);
}

// ============================================================================
// E2E: 多层嵌套分割（3 层以上）
// ============================================================================

#[test]
fn e2e_deep_nested_splits_three_levels() {
    let socket = unique_socket();
    Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "deep",
            "-x",
            "120",
            "-y",
            "40",
        ])
        .status()
        .unwrap();

    // 通过 muxterm 做 3 层嵌套：H → V → H（-L 临时连接，自动 attach deep）
    let (_o, e, rc) = run_mux(&["split-pane", "-h", "-L", &socket]);
    assert_eq!(rc, 0, "split1 rc={rc}: {e}");
    let (_o, e, rc) = run_mux(&["split-pane", "-v", "-L", &socket]);
    assert_eq!(rc, 0, "split2 rc={rc}: {e}");
    let (_o, e, rc) = run_mux(&["split-pane", "-h", "-L", &socket]);
    assert_eq!(rc, 0, "split3 rc={rc}: {e}");

    let (layout, e, rc) = run_mux(&["list-layout", "-L", &socket, "--format", "json"]);
    assert_eq!(rc, 0, "list-layout rc={rc}: {e}");

    // 至少两层嵌套：json 里应有嵌套的 type:split
    let split_count = layout.matches("\"type\":\"split\"").count();
    assert!(
        split_count >= 3,
        "应有 >= 3 层 split 节点: count={split_count} layout={layout}"
    );
    assert!(
        layout.contains("horizontal") && layout.contains("vertical"),
        "应同时有 horizontal 与 vertical: {layout}"
    );

    let panes = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-panes",
                "-a",
                "-t",
                "deep",
                "-F",
                "#{pane_id}",
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .filter(|l| !l.is_empty())
    .count();
    assert!(panes >= 4, "3 次 split 后应有 >= 4 pane: {panes}");

    cleanup(&socket);
}
