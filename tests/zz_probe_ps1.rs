//! 临时 probe：模拟 CI 多行 prompt 环境，验证 detach/reattach 后 buffer。
use std::time::{Duration, Instant};

use muxterm::core::runtime::herdr::observe::{channel, ObserveStream, StreamMode};
use muxterm::core::runtime::herdr::session::HerdrSession;
use muxterm::core::types::PaneId;
use support::herdr_test_support::IsolatedHerdr;

mod support;

fn wait_server_text(session: &HerdrSession, wire_pane: &str, text: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(bytes) = session.pane_read_recent_ansi_lines(wire_pane, 2000) {
            if String::from_utf8_lossy(&bytes).contains(text) {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn probe_prompt_env_reattach() {
    let herdr = IsolatedHerdr::start("probe-ps1");
    let (_ws, _tab, wire_pane) = herdr.create_workspace("/tmp", "probe-ps1-ws");
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

    // 模拟 CI 多行带色 prompt
    s1.send_input(b"export PS1='\\[\\e[38;5;2m\\]\\u@\\h:\\w\\[\\e[0m\\]\ncal-$$$ '\r")
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let token = format!("TK_PS1_{}", std::process::id());
    s1.send_input(format!("printf '{}'\r", token).as_bytes())
        .unwrap();
    assert!(wait_server_text(&sess, &wire_pane, &token), "B 阶段 token");
    println!("PS1_SET token_present=true");
    drop(s1);
    std::thread::sleep(Duration::from_millis(300));

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
    println!("REATTACH_PS1 token_preserved={ok}");
    drop(s2);
    std::thread::sleep(Duration::from_millis(200));
}
