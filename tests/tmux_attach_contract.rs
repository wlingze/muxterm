//! W13 attach 契约 — core 层（无 GTK）。
//!
//! 先在隔离 tmux 铺 2tab/3pane 和 token，再 `TmuxRuntime::new_with_attach`。
//! 空 session echo 不算。失败时拆更小单测，但本文件最终必须绿。

mod support;

use std::time::{Duration, Instant};

use muxterm::core::model::TerminalModel;
use muxterm::core::runtime::TmuxRuntime;
use support::tmux_test_support::{respawn_cup_flood, tmux_available};
use support::workspace_attach_contract::{
    assert_core_painted_topology, build_painted_2tab_3pane, count_pane_output_events,
    ATTACH_TIMEOUT, CUP_FLOOD_FRAMES, MAX_OUTPUT_EVENTS_PER_SEC,
};

fn connect_attach(socket: &str, session: &str) -> (TerminalModel, tokio::runtime::Runtime) {
    let runtime = TmuxRuntime::new_with_attach(Some(socket), session);
    let mut model = TerminalModel::new(Box::new(runtime));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(model.connect())
        .expect("attach connect 失败（隔离 socket，不是用户默认 server）");
    let _ = model.poll_events();
    (model, rt)
}

fn wait_topology(model: &mut TerminalModel) {
    let deadline = Instant::now() + ATTACH_TIMEOUT;
    while Instant::now() < deadline {
        let _ = model.refresh();
        if model.state().tabs().len() >= 2 {
            let active = model.state().tabs().iter().find(|t| t.active).map(|t| t.id);
            if let Some(tab) = active {
                if model
                    .state()
                    .layout(&tab)
                    .map(|l| l.tree.leaves().len())
                    .unwrap_or(0)
                    == 3
                {
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// attach 已有 2tab/3pane：core 必须有布局和播种 token（白屏 = 快照没进缓冲）。
#[test]
fn attach_preexist_2tab_3pane_seeds_core_buffers() {
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    let painted = build_painted_2tab_3pane("core-seed");
    let (mut model, rt) = connect_attach(&painted.socket, &painted.session);
    wait_topology(&mut model);
    assert_core_painted_topology(model.state(), &painted);
    let _ = rt.block_on(model.shutdown());
}

/// CUP 洪水不得把事件队列打满当直播；1s 内 PaneOutput 有上界（1820.log）。
#[test]
fn attach_cup_flood_bounds_pane_output_events() {
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    let painted = build_painted_2tab_3pane("core-flood");
    let (mut model, rt) = connect_attach(&painted.socket, &painted.session);
    wait_topology(&mut model);
    assert_core_painted_topology(model.state(), &painted);

    let target = painted.pane_target(painted.tab1_panes[0]);
    respawn_cup_flood(&painted.socket, &target, CUP_FLOOD_FRAMES);

    let window = Duration::from_secs(1);
    let start = Instant::now();
    let mut n = 0usize;
    while start.elapsed() < window {
        let events = model.refresh();
        n += count_pane_output_events(&events);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        n <= MAX_OUTPUT_EVENTS_PER_SEC,
        "1s 内 PaneOutput={n} > {MAX_OUTPUT_EVENTS_PER_SEC}：必须 %pause 或合并（1820.log pane 39 无 pause）。"
    );

    let _ = model.refresh();
    let mut blob = String::new();
    for tab in model.state().tabs() {
        for pane in model.state().panes(&tab.id) {
            if let Some(bytes) = model.state().pane_output(&pane.id) {
                blob.push_str(&String::from_utf8_lossy(bytes));
            }
        }
    }
    assert!(
        blob.contains("FLOOD_DONE") || blob.contains("frame-"),
        "洪水后 core 缓冲应留下末帧，不能被裁成空。len={}",
        blob.len()
    );

    let _ = rt.block_on(model.shutdown());
}
