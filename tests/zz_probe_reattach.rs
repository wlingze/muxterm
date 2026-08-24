//! 临时 probe：验证 herdr detach/reattach 尺寸组合对服务端 buffer 的影响。
//! 只用隔离 session（muxterm-test-probe-re2-<pid>），绝不碰用户会话。
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

fn attach_once(
    sess: &HerdrSession,
    pane: &str,
    cols: u16,
    rows: u16,
    mode: StreamMode,
) -> ObserveStream {
    let (tx, _rx) = channel();
    ObserveStream::start(
        sess.client_socket_path(),
        pane,
        PaneId(1),
        1,
        mode,
        false,
        cols,
        rows,
        tx,
    )
    .expect("流启动失败")
}

fn attach(
    sess: &HerdrSession,
    pane: &str,
    cols: u16,
    rows: u16,
    mode: StreamMode,
) -> ObserveStream {
    attach_once(sess, pane, cols, rows, mode)
}

fn scenario(label: &str, first: (u16, u16), reopen: (u16, u16), mode: StreamMode) {
    let herdr = IsolatedHerdr::start("probe-re2");
    let (_ws, _tab, wire_pane) = herdr.create_workspace("/tmp", "probe-re2-ws");
    let sess = HerdrSession::new(herdr.name(), herdr.socket_path());
    let token = format!("TK_{label}_{}", std::process::id());

    let mut s1 = attach_once(&sess, &wire_pane, first.0, first.1, StreamMode::Control);
    s1.send_input(format!("printf '{}'\r", token).as_bytes())
        .unwrap();
    assert!(
        wait_server_text(&sess, &wire_pane, &token),
        "{label} B 阶段 token"
    );
    drop(s1);
    std::thread::sleep(Duration::from_millis(300));

    let s2 = attach_once(&sess, &wire_pane, reopen.0, reopen.1, mode);
    let ok = wait_server_text(&sess, &wire_pane, &token);
    println!("{label}: first={first:?} reopen={reopen:?} mode={mode:?} token_preserved={ok}");
    drop(s2);
    std::thread::sleep(Duration::from_millis(200));
}

#[test]
fn probe_reattach_size_matrix() {
    scenario("same-size", (86, 20), (86, 20), StreamMode::Observe);
    scenario("shrink", (86, 20), (54, 23), StreamMode::Observe);
    scenario("grow", (54, 23), (86, 20), StreamMode::Observe);
    scenario("big-shrink", (120, 40), (54, 23), StreamMode::Observe);
    scenario("big-to-big", (120, 40), (86, 20), StreamMode::Observe);
}
