//! W14 功能契约 — core（无 GTK）：搜索 / OSC133 / mock-codex / tail-f。
//!
//! 真 tmux 字节必须进 PaneBuf。Mock PaneBuf 注入不算。

mod support;

use std::time::{Duration, Instant};

use muxterm::core::attention::signal::AttentionSignal;
use muxterm::core::runtime::TmuxRuntime;
use muxterm::core::types::PaneId;
use muxterm::core::workspace::id::WorkspaceId;
use muxterm::core::workspace::workspace::Workspace;
use support::feature_e2e_contract::*;
use support::tmux_test_support::{tmux_available, wait_capture_contains};

fn connect_workspace(socket: &str, session: &str) -> (Workspace, tokio::runtime::Runtime) {
    let id = WorkspaceId::new("local", None, session, "tmux", "");
    let runtime = TmuxRuntime::new_with_attach(Some(socket), session);
    let mut ws = Workspace::new(id, session.to_string(), Box::new(runtime));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(ws.connect())
        .expect("attach 失败（隔离 socket）");
    (ws, rt)
}

fn wait_search_hit(ws: &mut Workspace, token: &str) {
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    while Instant::now() < deadline {
        let _ = ws.refresh();
        if !ws.search_workspace(token).is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let tab_ids: Vec<_> = ws.state().tabs().iter().map(|t| t.id).collect();
    let mut sample = String::new();
    for tid in tab_ids {
        let pane_ids: Vec<_> = ws.state().panes(&tid).into_iter().map(|p| p.id).collect();
        for pid in pane_ids {
            let preview: String = ws.pane_text(pid).chars().take(80).collect();
            sample.push_str(&format!("@{}:{preview} | ", pid.0));
        }
    }
    panic!("attach 后 PaneBuf 搜索不到 {token}（播种/搜索回归）。{sample}");
}

#[test]
fn search_finds_token_painted_before_attach() {
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    let fx = build_two_pane_cat("core-search");
    let (mut ws, rt) = connect_workspace(&fx.socket, &fx.session);
    wait_search_hit(&mut ws, &fx.search_token);
    let hits = ws.search_workspace(&fx.search_token);
    assert_eq!(hits.len(), 1, "搜索命中应恰好一条: {hits:?}");
    assert!(
        hits[0].line.contains(&fx.search_token),
        "命中行应含 token: {}",
        hits[0].line
    );
    let _ = rt.block_on(ws.shutdown());
}

#[test]
fn background_osc133_done_is_attention_signal() {
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    let fx = build_two_pane_cat("core-osc");
    let (mut ws, rt) = connect_workspace(&fx.socket, &fx.session);
    wait_search_hit(&mut ws, &fx.bg_token);
    send_background_task_done(&fx.socket, &fx.pane_target(1));
    send_background_bel(&fx.socket, &fx.pane_target(1));
    let pane = PaneId(fx.panes[1]);
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    let mut saw_done = false;
    let mut saw_bel = false;
    while Instant::now() < deadline {
        let _ = ws.refresh();
        for sig in ws.take_attention_signals(pane) {
            match sig {
                AttentionSignal::CommandDone { .. } => saw_done = true,
                AttentionSignal::AttentionRequest { .. } => saw_bel = true,
                _ => {}
            }
        }
        if saw_done && saw_bel {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(saw_done, "后台 pane 的 OSC 133 D 必须变成 CommandDone 信号");
    assert!(saw_bel, "后台 pane 的 BEL 必须变成 AttentionRequest");
    let _ = rt.block_on(ws.shutdown());
}

#[test]
fn mock_codex_last_frame_reaches_pane_buffer() {
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    if !mock_codex_py().is_file() {
        panic!("tests/scripts/mock_codex.py 必须存在");
    }
    let fx = build_two_pane_cat("core-codex");
    let (mut ws, rt) = connect_workspace(&fx.socket, &fx.session);
    wait_search_hit(&mut ws, &fx.search_token);
    respawn_mock_codex(&fx.socket, &fx.pane_target(0));
    wait_capture_contains(
        &fx.socket,
        &fx.pane_target(0),
        "MOCK_CODEX_DONE",
        FEATURE_TIMEOUT,
    );
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    let mut blob = String::new();
    while Instant::now() < deadline {
        let _ = ws.refresh();
        blob = ws.pane_text(PaneId(fx.panes[0]));
        if blob.contains("TOKEN_HEADER")
            && blob.contains("TOKEN_PROMPT")
            && (blob.contains("MOCK_CODEX_DONE") || blob.contains("MOCK_CODEX_FRAME="))
        {
            let _ = rt.block_on(ws.shutdown());
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "mock-codex 末帧必须进 PaneBuf（白屏/只吃半帧）。blob 长度 {}",
        blob.len()
    );
}

#[test]
fn tail_f_appends_reach_pane_buffer() {
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    let fx = build_two_pane_cat("core-tail");
    let (mut ws, rt) = connect_workspace(&fx.socket, &fx.session);
    wait_search_hit(&mut ws, &fx.search_token);
    let log = std::env::temp_dir().join(format!("muxterm-tail-{}.log", fx.session));
    let _ = std::fs::remove_file(&log);
    append_line(&log, "TAIL_BOOT");
    start_tail_f(&fx.socket, &fx.pane_target(0), &log);
    wait_capture_contains(&fx.socket, &fx.pane_target(0), "TAIL_BOOT", FEATURE_TIMEOUT);
    append_line(&log, "TAIL_FOLLOW_TOKEN");
    wait_capture_contains(
        &fx.socket,
        &fx.pane_target(0),
        "TAIL_FOLLOW_TOKEN",
        FEATURE_TIMEOUT,
    );
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    while Instant::now() < deadline {
        let _ = ws.refresh();
        let text = ws.pane_text(PaneId(fx.panes[0]));
        if text.contains("TAIL_FOLLOW_TOKEN") {
            let _ = std::fs::remove_file(&log);
            let _ = rt.block_on(ws.shutdown());
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_file(&log);
    panic!("tail -f 追加行必须进 PaneBuf，不能只停在启动那一截");
}

/// 源码必须有可诊断的 tracing target（1820.log 只有洪水 %output，找不到播种/pause）。
#[test]
fn debug_log_targets_exist_in_source() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let required = [
        ("src/core/runtime/tmux/backend.rs", "muxterm::tmux::seed"),
        ("src/core/runtime/tmux/backend.rs", "muxterm::tmux::pause"),
        ("src/platform/linux/layout_host.rs", "muxterm::layout"),
        ("src/platform/linux/pane_view.rs", "muxterm::surface"),
        ("src/core/workspace/workspace.rs", "muxterm::search"),
        ("src/platform/linux/window.rs", "muxterm::notify"),
    ];
    for (rel, needle) in required {
        let text = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
        assert!(
            text.contains(needle),
            "{rel} 必须有 tracing target `{needle}`（attach 白屏/高 CPU 时才能在日志里定位，禁止只打每条 %output）"
        );
    }
}

/// 1820.log 把每条 %output 打成 DEBUG，13MB 仍看不出播种/pause。该文案不得再走 debug!。
#[test]
fn live_output_must_not_debug_every_fragment() {
    let text = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/core/runtime/tmux/backend.rs"),
    )
    .expect("backend.rs");
    let needle = "实时 %output 交付";
    if let Some(idx) = text.find(needle) {
        let start = idx.saturating_sub(180);
        let window = &text[start..idx];
        assert!(
            !window.contains("tracing::debug!"),
            "「实时 %output 交付」不得再用 tracing::debug!（1820.log 洪水）。改 TRACE 或按 pane 限速，并打 muxterm::tmux::seed / muxterm::tmux::pause"
        );
    }
}
