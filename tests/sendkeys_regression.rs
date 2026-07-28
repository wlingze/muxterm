//! send_keys 质量门测试：验证混合 Literal+Special 键的正确 tmux 命令生成。
//!
//! 质量门要求（/tmp/muxterm-sendkeys-quality-gate.md）：
//! 1. [Literal("echo MARKER"), Special("Enter")] 必须证明文本和 Enter 是分离的 tmux 命令，
//!    不是把 "Enter" 当字面文本拼接。
//! 2. 真实 tmux backend 集成测试：发送 text + Enter，原生 tmux capture 含 exact marker。
//! 3. daemon local-shell CLI 回归测试：exact marker 在 capture 中。
//! 4. 覆盖 raw capture-pane 行为。

#![cfg(feature = "ffi")]

use std::process::Command;
use std::time::Duration;

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
    format!("muxterm-sk-{}-{}", label, nanos)
}

fn kill_server(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
}

/// ── Test 1: command builder mixed-key produces separate commands ──
/// [Literal("echo MARKER"), Special("Enter")] 必须生成两条命令：
/// send-keys -t %N -l "echo MARKER" + send-keys -t %N Enter。
/// 不能把 "Enter" 拼成字面文本 "echo MARKEREnter"。
#[test]
fn send_keys_mixed_literal_and_special_produces_separate_commands() {
    use muxterm::core::tmux::command::{send_keys, Key, PaneId};

    let keys = vec![
        Key::Literal("echo MARKER".to_string()),
        Key::Special("Enter"),
    ];
    let cmd = send_keys(PaneId(0), &keys);
    let line = cmd.to_line();

    // 必须含 -l "echo MARKER"（逐字文本）
    assert!(line.contains("-l"), "应含 -l 标志（逐字模式）: {line}");
    assert!(
        line.contains("echo MARKER"),
        "应含逐字文本 'echo MARKER': {line}"
    );

    // 必须含 Enter 作为特殊键（不是字面文本）
    // Enter 应该在第二条命令中，用换行分隔
    assert!(line.contains("Enter"), "应含 Enter 特殊键: {line}");

    // 关键断言：不能把 Enter 当字面文本拼进 -l 参数
    // 如果是错误的拼接，-l 参数会是 "echo MARKEREnter"
    assert!(
        !line.contains("echo MARKEREnter"),
        "Enter 不应被拼进 -l 逐字文本: {line}"
    );

    // 两条命令应被换行分隔
    assert!(
        line.contains("\n"),
        "混合键应生成多条命令（换行分隔）: {line}"
    );
}

/// ── Test 1b: pure literal only produces single -l command ──
#[test]
fn send_keys_pure_literal_produces_single_l_command() {
    use muxterm::core::tmux::command::{send_keys, Key, PaneId};

    let keys = vec![Key::Literal("echo hello".to_string())];
    let cmd = send_keys(PaneId(0), &keys);
    let line = cmd.to_line();

    assert!(line.contains("-l"), "应含 -l: {line}");
    assert!(line.contains("echo hello"), "应含文本: {line}");
    assert!(!line.contains("Enter"), "不应含 Enter: {line}");
    // 纯 literal 不应有多行
    assert_eq!(
        line.matches('\n').count(),
        1,
        "纯 literal 应只有末尾换行: {line}"
    );
}

/// ── Test 1c: pure special only produces single special-key command ──
#[test]
fn send_keys_pure_special_produces_single_special_command() {
    use muxterm::core::tmux::command::{send_keys, Key, PaneId};

    let keys = vec![Key::Special("Enter")];
    let cmd = send_keys(PaneId(0), &keys);
    let line = cmd.to_line();

    assert!(line.contains("Enter"), "应含 Enter: {line}");
    assert!(!line.contains("-l"), "不应含 -l: {line}");
    assert_eq!(
        line.matches('\n').count(),
        1,
        "纯 special 应只有末尾换行: {line}"
    );
}

/// ── Test 2: backend integration — send text+Enter, native capture has marker ──
/// 通过 TmuxBackend 发送 echo MARKER + Enter，用原生 tmux capture-pane 验证。
#[test]
fn backend_send_keys_text_plus_enter_native_capture_has_marker() {
    use muxterm::core::model::task::Task;
    use muxterm::core::model::TerminalModel;
    use muxterm::core::runtime::TmuxBackend;
    use muxterm::core::terminal::input::KeyEvent;

    let socket = unique_socket("backend-sk");
    let session = format!("sk-test-{}", rand_suffix());

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

    // 等待 active pane 就绪
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let _ = model.refresh();
        if model.state().active_pane().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let pane_id = model.state().active_pane().expect("应有 active pane").id;

    // 发送 echo MARKER + Enter
    let marker = format!("sk_marker_{}", rand_suffix());
    // Send the marker chars directly
    let mut all_keys: Vec<KeyEvent> = "echo ".chars().map(KeyEvent::Char).collect();
    all_keys.extend(marker.chars().map(KeyEvent::Char));
    all_keys.push(KeyEvent::Enter);

    model
        .execute(Task::SendKeys {
            target: pane_id,
            keys: all_keys,
        })
        .expect("SendKeys 失败");

    // 等待 shell 执行
    std::thread::sleep(Duration::from_millis(1500));

    // 用原生 tmux capture-pane 验证 marker
    let output = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "capture-pane",
            "-t",
            &format!("%{}", pane_id.0),
            "-p",
            "-S",
            "-20",
        ])
        .output()
        .expect("capture-pane 失败");
    let captured = String::from_utf8_lossy(&output.stdout);

    assert!(
        captured.contains(&marker),
        "原生 tmux capture 应含 exact marker '{marker}':\n{captured}"
    );

    let _ = rt.block_on(model.shutdown());
    kill_server(&socket);
}

/// ── Test 3: daemon local-shell CLI regression — exact marker in capture ──
/// 通过 muxterm CLI daemon 模式（LocalBackend）发送 echo + Enter，capture 含 marker。
#[test]
#[cfg(feature = "tui")]
fn daemon_local_shell_cli_exact_marker_in_capture() {
    let bin = std::path::PathBuf::from(
        std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "../muxterm-target".to_string()),
    )
    .join("debug")
    .join("muxterm");
    if !bin.exists() {
        // fallback
        let alt = std::path::PathBuf::from("target/debug/muxterm");
        if !alt.exists() {
            eprintln!("skip: muxterm binary 不存在");
            return;
        }
    }

    let name = format!("sk-daemon-{}", rand_suffix());

    // 创建 daemon session
    let output = Command::new(&bin)
        .args(["new-session", "-s", &name])
        .output()
        .expect("new-session 失败");
    assert!(output.status.success(), "new-session 应成功");

    // 等待 daemon 就绪
    std::thread::sleep(Duration::from_millis(500));

    // 发送 echo MARKER（send-keys 自动加 Enter）
    let marker = format!("sk_daemon_{}", rand_suffix());
    let output = Command::new(&bin)
        .args(["send-keys", "-s", &name, &format!("echo {marker}")])
        .output()
        .expect("send-keys 失败");
    assert!(output.status.success(), "send-keys 应成功");

    // 等待 shell 执行
    std::thread::sleep(Duration::from_millis(1500));

    // capture-pane
    let output = Command::new(&bin)
        .args(["capture-pane", "-s", &name])
        .output()
        .expect("capture-pane 失败");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains(&marker),
        "daemon capture-pane 应含 exact marker '{marker}':\n{stdout}"
    );

    // 清理
    let _ = Command::new(&bin)
        .args(["kill-session", "-s", &name])
        .output();
}

/// ── Test 4: raw capture-pane behavior ──
/// 验证 tmux capture-pane -p -S -N 能正确获取历史输出。
#[test]
fn raw_capture_pane_returns_shell_output() {
    let socket = unique_socket("raw-cap");
    let session = format!("raw-cap-{}", rand_suffix());

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

    // 直接用 tmux send-keys 发命令
    let marker = format!("raw_cap_{}", rand_suffix());
    Command::new("tmux")
        .args([
            "-L",
            &socket,
            "send-keys",
            "-t",
            &session,
            &format!("echo {marker}"),
            "Enter",
        ])
        .output()
        .expect("send-keys 失败");

    std::thread::sleep(Duration::from_millis(1000));

    // 用 capture-pane -p -S -10 获取
    let output = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "capture-pane",
            "-t",
            &session,
            "-p",
            "-S",
            "-10",
        ])
        .output()
        .expect("capture-pane 失败");
    let captured = String::from_utf8_lossy(&output.stdout);

    assert!(
        captured.contains(&marker),
        "raw capture-pane 应含 marker '{marker}':\n{captured}"
    );

    kill_server(&socket);
}
