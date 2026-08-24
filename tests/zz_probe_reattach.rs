//! 临时 probe：打印 pane 内 shell 环境 + 验证 detach/reattach 后 buffer。
//! 只用隔离 session（muxterm-test-probe-env-<pid>），绝不碰用户会话。
use std::time::{Duration, Instant};

use muxterm::core::runtime::herdr::observe::{channel, ObserveStream, StreamMode};
use muxterm::core::runtime::herdr::session::HerdrSession;
use muxterm::core::types::PaneId;
use support::herdr_test_support::IsolatedHerdr;

mod support;

fn read_text(session: &HerdrSession, wire_pane: &str) -> String {
    session
        .pane_read_recent_ansi_lines(wire_pane, 2000)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

fn wait_server_text(session: &HerdrSession, wire_pane: &str, text: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if read_text(session, wire_pane).contains(text) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn probe_env_and_reattach() {
    let herdr = IsolatedHerdr::start("probe-env");
    let (_ws, _tab, wire_pane) = herdr.create_workspace("/tmp", "probe-env-ws");
    let sess = HerdrSession::new(herdr.name(), herdr.socket_path());

    let (tx, _rx) = channel();
    let mut s1 = ObserveStream::start(
        sess.client_socket_path(),
        &wire_pane,
        PaneId(1),
        1,
        StreamMode::Control,
        false,
        54,
        23,
        tx.clone(),
    )
    .unwrap();

    // 1) 打印 pane 内 shell 环境（短命令，避免折行；先等 shell ready）
    std::thread::sleep(Duration::from_millis(1200));
    for cmd in [
        "echo PROBE_ENV_START",
        "echo 0=$0",
        "echo SHELL=$SHELL",
        "echo TERM=$TERM",
        "echo LANG=$LANG",
        "echo PS1=$PS1",
        "echo PSCMD=$PROMPT_COMMAND",
        "command -v zsh",
        "command -v bash",
        "echo PROBE_ENV_END",
    ] {
        s1.send_input(format!("{}\r", cmd).as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(120));
    }
    assert!(
        wait_server_text(&sess, &wire_pane, "PROBE_ENV_END"),
        "环境探测未完成"
    );
    std::thread::sleep(Duration::from_millis(300));
    println!("=== PANE ENV ===\n{}", read_text(&sess, &wire_pane));

    // 2) 出 token
    let token = format!("TK_ENV_{}", std::process::id());
    s1.send_input(format!("printf '{}'\r", token).as_bytes()).unwrap();
    assert!(wait_server_text(&sess, &wire_pane, &token), "B 阶段 token");
    println!("BEFORE_DETACH token_present=true");
    drop(s1);
    std::thread::sleep(Duration::from_millis(300));

    // 3) detach 后
    let after_detach = read_text(&sess, &wire_pane).contains(&token);
    println!("AFTER_DETACH token_preserved={after_detach}");

    // 4) 同尺寸 reattach
    let s2 = ObserveStream::start(
        sess.client_socket_path(),
        &wire_pane,
        PaneId(1),
        1,
        StreamMode::Observe,
        false,
        54,
        23,
        tx,
    )
    .unwrap();
    let ok = wait_server_text(&sess, &wire_pane, &token);
    println!("AFTER_REATTACH token_preserved={ok}");
    if !ok {
        println!("=== REATTACH 后内容 ===\n{}", read_text(&sess, &wire_pane));
    }
    drop(s2);
    std::thread::sleep(Duration::from_millis(200));
}
