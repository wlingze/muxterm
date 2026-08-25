//! Herdr detach/reattach 服务端内容保留的 wire 级验证。
//!
//! 与 GTK e2e 的 detach_reattach 场景不同，这里不走 AppWindow，直接
//! 用 ObserveStream（wire 协议）attach → 写 token → detach → 重新
//! attach（不同尺寸）→ pane.read(recent) 必须仍能看到 token。
//! 用于区分「herdr 0.8.0 服务端行为」与「Muxterm GUI 层问题」。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use muxterm::core::runtime::herdr::observe::{channel, ObserveStream, StreamMode};
use muxterm::core::runtime::herdr::session::HerdrSession;
use muxterm::core::types::PaneId;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};

const TOKEN: &str = "KEEP_TOKEN_XYZ";

/// 轮询 pane.read(recent) 直到出现 needle（最多 10s）。
fn wait_server_text(session: &HerdrSession, pane: &str, needle: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(bytes) = session.pane_read_recent_ansi_lines(pane, 2000) {
            if String::from_utf8_lossy(&bytes).contains(needle) {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn direct_reattach_preserves_server_scrollback() {
    if !herdr_available() {
        eprintln!("herdr 不可用，跳过 direct reattach 验证");
        return;
    }
    let herdr = IsolatedHerdr::start("direct-reattach");
    let (_workspace_id, _tab, wire_pane) = herdr.create_workspace("/tmp", "direct-reattach");
    let session = HerdrSession::new(herdr.name(), herdr.socket_path());

    let (tx, _rx) = channel();
    // 1) 初始 attach（CI 尺寸 63x40），**命令写在 63x40 下**（B 阶段）。
    let mut stream1 = ObserveStream::start(
        session.client_socket_path(),
        &wire_pane,
        PaneId(1),
        1,
        StreamMode::Control,
        false,
        63,
        40,
        tx.clone(),
    )
    .expect("初始 control 流启动失败");
    stream1
        .send_input(b"printf 'KEEP_TOKEN_XYZ\\n'\r")
        .expect("写命令失败");
    assert!(
        wait_server_text(&session, &wire_pane, TOKEN),
        "B 阶段后服务端必须持有 token"
    );

    // 2) replace：新流 attach（27x23，takeover 旧流），旧流随即 Drop。
    //    模拟 Muxterm 在 build_scenario 阶段对 pane 的流替换。
    let stream2 = ObserveStream::start(
        session.client_socket_path(),
        &wire_pane,
        PaneId(1),
        1,
        StreamMode::Observe,
        false,
        27,
        23,
        tx.clone(),
    )
    .expect("replace 流启动失败");
    drop(stream1);
    std::thread::sleep(Duration::from_millis(300));

    // 3) detach：关闭全部流（模拟 Task::Detach 关流）。
    drop(stream2);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        wait_server_text(&session, &wire_pane, TOKEN),
        "detach 后服务端必须仍持有 token"
    );

    // 4) reopen：新流 attach（27x23，**从 63x40 缩小**）。
    //    若缩小 resize 触发 ghostty 清 scrollback，token 会丢。
    let mut stream3 = ObserveStream::start(
        session.client_socket_path(),
        &wire_pane,
        PaneId(1),
        1,
        StreamMode::Observe,
        false,
        27,
        23,
        tx.clone(),
    )
    .expect("reopen 流启动失败");
    assert!(
        wait_server_text(&session, &wire_pane, TOKEN),
        "reopen 缩小 attach 不得清空 scrollback"
    );
    // 5) reopen 后 Muxterm 把 observe 流 resize 回 pane 实际尺寸（放大）。
    //    CI 日志：reopen attach 27x23 后 resize 63x40。缩小+放大两次
    //    reflow 可能把 scrollback 中间行挤掉（输出行丢失的嫌疑）。
    stream3.resize(63, 40).expect("reopen 后 resize 失败");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        wait_server_text(&session, &wire_pane, TOKEN),
        "reopen 缩小后放大 resize 不得清空 scrollback"
    );
    // 6) verify 阶段 Muxterm 会 pane.focus（SwitchPane API）：确认 focus
    //    不会触发服务端 reflow 丢内容。
    session
        .call("pane.focus", serde_json::json!({ "pane_id": wire_pane }))
        .expect("pane.focus 失败");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        wait_server_text(&session, &wire_pane, TOKEN),
        "reopen 后 pane.focus 不得清空 scrollback"
    );
    drop(stream3);
    std::thread::sleep(Duration::from_millis(200));
}

/// A colored, multiline shell prompt must not prevent a reattached pane from
/// accepting a new command.  This is intentionally separate from the plain
/// scrollback test so prompt parsing and lifecycle continuity fail distinctly.
#[test]
fn direct_reattach_colored_prompt_accepts_new_command() {
    if !herdr_available() {
        eprintln!("herdr 不可用，跳过 colored prompt reattach 验证");
        return;
    }
    let herdr = IsolatedHerdr::start("colored-prompt-reattach");
    let (_workspace_id, _tab, wire_pane) = herdr.create_workspace("/tmp", "colored-prompt");
    let session = HerdrSession::new(herdr.name(), herdr.socket_path());
    let (tx, _rx) = channel();

    let mut first = ObserveStream::start(
        session.client_socket_path(),
        &wire_pane,
        PaneId(1),
        1,
        StreamMode::Control,
        false,
        54,
        23,
        tx.clone(),
    )
    .expect("colored prompt 初始流启动失败");
    first
        .send_input(b"export PS1='\\[\\e[38;5;2m\\]\\u@\\h\\[\\e[0m\\]\\ncal-$$$ '\r")
        .expect("设置 colored multiline PS1 失败");
    first
        .send_input(b"printf 'PROMPT_OLD\n'\r")
        .expect("写 old token 失败");
    assert!(
        wait_server_text(&session, &wire_pane, "PROMPT_OLD"),
        "colored prompt 阶段 old token 未到达服务端"
    );
    drop(first);
    std::thread::sleep(Duration::from_millis(300));

    let mut second = ObserveStream::start(
        session.client_socket_path(),
        &wire_pane,
        PaneId(1),
        1,
        StreamMode::Control,
        false,
        54,
        23,
        tx,
    )
    .expect("colored prompt reattach 流启动失败");
    second
        .send_input(b"printf 'PROMPT_NEW\n'\r")
        .expect("写 new token 失败");
    assert!(
        wait_server_text(&session, &wire_pane, "PROMPT_NEW"),
        "colored prompt reattach 后 new token 未到达服务端"
    );
    let text = String::from_utf8_lossy(
        &session
            .pane_read_recent_ansi_lines(&wire_pane, 2000)
            .expect("读取 colored prompt reattach 内容失败"),
    )
    .into_owned();
    assert!(text.contains("PROMPT_OLD"), "reattach 后 old token 丢失");
    assert!(text.contains("PROMPT_NEW"), "reattach 后 new token 丢失");
    assert!(text.contains("cal-"), "colored multiline prompt 未保留");
    drop(second);
}

/// 多 pane 并发 reopen：GTK e2e 是 4 pane 同时重新 attach，而单 pane
/// wire 测试在 CI 上通过。此测试用 2 个 pane 模拟并发 reopen，验证
/// 并发 attach 不会触发服务端 scrollback 竞态丢内容。
#[test]
fn concurrent_two_pane_reopen_preserves_scrollback() {
    if !herdr_available() {
        eprintln!("herdr 不可用，跳过并发 reopen 验证");
        return;
    }
    let herdr = IsolatedHerdr::start("concurrent-reopen");
    let (_workspace_id, _tab, wire_pane1) = herdr.create_workspace("/tmp", "concurrent-reopen");
    let session = HerdrSession::new(herdr.name(), herdr.socket_path());

    // 建第二个 pane（right split）。
    let split = session
        .call(
            "pane.split",
            serde_json::json!({ "pane_id": wire_pane1, "direction": "right" }),
        )
        .expect("pane.split 失败");
    let wire_pane2 = split
        .get("pane")
        .and_then(|p| p.get("pane_id"))
        .and_then(serde_json::Value::as_str)
        .expect("pane.split 响应缺 pane_id")
        .to_string();

    let token1 = "KEEP_TOKEN_PANE1";
    let token2 = "KEEP_TOKEN_PANE2";

    let (tx, _rx) = channel();
    // 1) 两个 pane 都 attach（63x40）并写 token。
    let mut s1 = ObserveStream::start(
        session.client_socket_path(),
        &wire_pane1,
        PaneId(1),
        1,
        StreamMode::Control,
        false,
        63,
        40,
        tx.clone(),
    )
    .expect("pane1 初始流失败");
    let mut s2 = ObserveStream::start(
        session.client_socket_path(),
        &wire_pane2,
        PaneId(2),
        1,
        StreamMode::Control,
        false,
        63,
        40,
        tx.clone(),
    )
    .expect("pane2 初始流失败");
    s1.send_input(b"printf 'KEEP_TOKEN_PANE1\\n'\r")
        .expect("pane1 写命令失败");
    s2.send_input(b"printf 'KEEP_TOKEN_PANE2\\n'\r")
        .expect("pane2 写命令失败");
    assert!(
        wait_server_text(&session, &wire_pane1, token1),
        "pane1 B 阶段缺 token"
    );
    assert!(
        wait_server_text(&session, &wire_pane2, token2),
        "pane2 B 阶段缺 token"
    );

    // 2) detach：两个流都关。
    drop(s1);
    drop(s2);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        wait_server_text(&session, &wire_pane1, token1),
        "detach 后 pane1 缺 token"
    );
    assert!(
        wait_server_text(&session, &wire_pane2, token2),
        "detach 后 pane2 缺 token"
    );

    // 3) **并发 reopen**：两个新流几乎同时 attach（27x23 缩小）。
    let mut r1 = ObserveStream::start(
        session.client_socket_path(),
        &wire_pane1,
        PaneId(1),
        1,
        StreamMode::Observe,
        false,
        27,
        23,
        tx.clone(),
    )
    .expect("pane1 reopen 流失败");
    let mut r2 = ObserveStream::start(
        session.client_socket_path(),
        &wire_pane2,
        PaneId(2),
        1,
        StreamMode::Observe,
        false,
        27,
        23,
        tx.clone(),
    )
    .expect("pane2 reopen 流失败");
    // 4) reopen 后 resize 回实际尺寸（放大 63x40），模拟 Muxterm 同步。
    r1.resize(63, 40).expect("pane1 reopen resize 失败");
    r2.resize(63, 40).expect("pane2 reopen resize 失败");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        wait_server_text(&session, &wire_pane1, token1),
        "并发 reopen 后 pane1 缺 token（多 pane attach 竞态？）"
    );
    assert!(
        wait_server_text(&session, &wire_pane2, token2),
        "并发 reopen 后 pane2 缺 token（多 pane attach 竞态？）"
    );
    drop(r1);
    drop(r2);
    std::thread::sleep(Duration::from_millis(200));
}
