//! W13 attach 契约 — core 层（无 GTK）。
//!
//! 先在隔离 tmux 铺 2tab/3pane 和 token，再 `TmuxRuntime::new_with_attach`。
//! 空 session echo 不算。失败时拆更小单测，但本文件最终必须绿。

mod support;

use std::time::{Duration, Instant};

use muxterm::test_support::core::protocol::state::StateChange;
use muxterm::test_support::core::runtime::tmux::backend::TmuxRuntime;
use muxterm::test_support::core::workspace::TerminalModel;
use support::tmux_test_support::{respawn_cup_flood, tmux_available};
use support::workspace_attach_contract::{
    assert_core_painted_topology, build_painted_2tab_3pane, count_pane_output_events,
    ATTACH_TIMEOUT, CUP_FLOOD_FRAMES, MAX_OUTPUT_EVENTS_PER_SEC,
};

fn connect_attach(
    socket: &str,
    session: &str,
) -> (TerminalModel, tokio::runtime::Runtime, Vec<StateChange>) {
    let mut runtime = TmuxRuntime::new_with_attach(Some(socket), session);
    // A real frontend allocates the viewport before requesting attach pixels.
    runtime.set_client_size(80, 24);
    let mut model = TerminalModel::new(Box::new(runtime));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(model.connect())
        .expect("attach connect 失败（隔离 socket，不是用户默认 server）");
    let initial = model.poll_events();
    (model, rt, initial)
}

fn collect_pixels(events: &[StateChange], surface: &mut String, index: &mut String) {
    for event in events {
        match event {
            StateChange::PaneOutput { data, .. }
            | StateChange::PaneSnapshot { data, .. }
            | StateChange::PaneFrame { data, .. } => {
                surface.push_str(&String::from_utf8_lossy(data));
            }
            StateChange::PaneIndexSnapshot { data, .. } => {
                index.push_str(&String::from_utf8_lossy(data));
            }
            _ => {}
        }
    }
}

fn wait_painted_topology(
    model: &mut TerminalModel,
    painted: &support::workspace_attach_contract::PaintedWorkspace,
    initial: &[StateChange],
) {
    let deadline = Instant::now() + ATTACH_TIMEOUT;
    let mut surface = String::new();
    let mut index = String::new();
    collect_pixels(initial, &mut surface, &mut index);
    while Instant::now() < deadline {
        collect_pixels(&model.refresh(), &mut surface, &mut index);
        if model.state().tabs().len() >= 2 {
            let active = model.state().tabs().iter().find(|t| t.active).map(|t| t.id);
            if let Some(tab) = active {
                if model
                    .state()
                    .layout(&tab)
                    .map(|l| l.tree.leaves().len())
                    .unwrap_or(0)
                    == 3
                    && painted
                        .tab1_tokens
                        .iter()
                        .all(|token| surface.contains(token))
                    && (surface.contains(&painted.tab2_token)
                        || index.contains(&painted.tab2_token))
                {
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_core_painted_topology(model.state(), painted);
    for token in &painted.tab1_tokens {
        assert!(
            surface.contains(token),
            "visible pane seed missing from Surface: {token}; bytes={}",
            surface.len()
        );
    }
    assert!(
        surface.contains(&painted.tab2_token) || index.contains(&painted.tab2_token),
        "hidden pane seed missing: {}; surface={} index={}",
        painted.tab2_token,
        surface.len(),
        index.len()
    );
}

/// attach 已有 2tab/3pane：core 必须有布局和播种 token（白屏 = 快照没进缓冲）。
#[test]
fn attach_preexist_2tab_3pane_seeds_core_buffers() {
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    let painted = build_painted_2tab_3pane("core-seed");
    let (mut model, rt, initial) = connect_attach(&painted.socket, &painted.session);
    wait_painted_topology(&mut model, &painted, &initial);
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
    let (mut model, rt, initial) = connect_attach(&painted.socket, &painted.session);
    wait_painted_topology(&mut model, &painted, &initial);
    assert_core_painted_topology(model.state(), &painted);

    let target = painted.pane_target(painted.tab1_panes[0]);
    respawn_cup_flood(&painted.socket, &target, CUP_FLOOD_FRAMES);

    let window = Duration::from_secs(1);
    let start = Instant::now();
    let mut n = 0usize;
    let mut flood_surface = String::new();
    let mut flood_index = String::new();
    while start.elapsed() < window {
        let events = model.refresh();
        n += count_pane_output_events(&events);
        collect_pixels(&events, &mut flood_surface, &mut flood_index);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        n <= MAX_OUTPUT_EVENTS_PER_SEC,
        "1s 内 PaneOutput={n} > {MAX_OUTPUT_EVENTS_PER_SEC}：必须 %pause 或合并（1820.log pane 39 无 pause）。"
    );

    collect_pixels(&model.refresh(), &mut flood_surface, &mut flood_index);
    assert!(
        flood_surface.contains("FLOOD_DONE") || flood_surface.contains("frame-"),
        "洪水后 Surface 应收到末帧，不能被裁成空。len={}",
        flood_surface.len()
    );

    let _ = rt.block_on(model.shutdown());
}
