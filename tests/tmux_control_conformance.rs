//! tmux control-mode conformance scenarios that exercise the lossy edge cases
//! covered by tmux's own control.c regressions: a stalled client, bounded pane
//! output, and live recovery without recapturing an already-open Surface.

mod support;

use std::time::{Duration, Instant};

use muxterm::core::model::state::StateChange;
use muxterm::core::model::TerminalModel;
use muxterm::core::runtime::TmuxRuntime;
use support::tmux_test_support::{
    create_session, kill_server, list_pane_ids, send_keys_line, tmux_available, unique_socket,
};

struct IsolatedServer(String);

impl Drop for IsolatedServer {
    fn drop(&mut self) {
        kill_server(&self.0);
    }
}

fn connected_model(socket: &str, session: &str) -> (TerminalModel, tokio::runtime::Runtime) {
    let mut model = TerminalModel::new(Box::new(TmuxRuntime::new_with_attach(
        Some(socket),
        session,
    )));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio runtime");
    runtime
        .block_on(model.connect())
        .expect("isolated tmux attach must connect");
    // Consume the initial connecting/attach snapshot. The test below verifies
    // that the deliberate stall does not generate another snapshot.
    let _ = model.poll_events();
    (model, runtime)
}

#[test]
fn stalled_attach_client_resumes_live_to_last_frame_without_recapture() {
    if !tmux_available() {
        eprintln!("skip: tmux unavailable");
        return;
    }
    let socket = unique_socket("control-resync");
    let _server = IsolatedServer(socket.clone());
    create_session(&socket, "resync", 80, 24);
    let pane = list_pane_ids(&socket, "resync")[0];
    let (mut model, runtime) = connected_model(&socket, "resync");

    // Stop polling long enough to fill the bounded pane-output lane. The reader
    // must keep parsing control boundaries, bound and coalesce pane data, then
    // resume the open Surface with live bytes. A large, fast burst makes this
    // deterministic even when tmux coalesces many short frames into a small
    // number of `%output` blocks.
    let script = "for i in $(seq 1 5000); do printf \"\\033[H\\033[2Jframe-%s\\n\" \"$i\"; done; printf \"FLOOD_DONE\\n\"; exec /bin/cat";
    send_keys_line(&socket, &format!("%{pane}"), script);
    // Do not wait for the shell to finish before polling: the control reader
    // intentionally stalls here while tmux continues running; once the first
    // poll drains the lane, live recovery must retain the eventual last frame.
    std::thread::sleep(Duration::from_secs(2));

    let deadline = Instant::now() + Duration::from_secs(6);
    let mut snapshots = 0usize;
    let mut saw_last_frame = false;
    let mut total_events = 0usize;
    let mut total_outputs = 0usize;
    let mut total_output_bytes = 0usize;
    let mut last_frame_seen_at = None;
    while Instant::now() < deadline {
        let events = model.refresh();
        total_events += events.len();
        total_outputs += events
            .iter()
            .filter(|event| matches!(event, StateChange::PaneOutput { .. }))
            .count();
        total_output_bytes += events
            .iter()
            .filter_map(|event| match event {
                StateChange::PaneOutput { data, .. } => Some(data.len()),
                _ => None,
            })
            .sum::<usize>();
        for event in events {
            if let StateChange::PaneSnapshot { pane: id, .. } = event {
                if id.0 == pane {
                    snapshots += 1;
                }
            }
        }
        saw_last_frame = model
            .state()
            .pane_output(&muxterm::core::types::PaneId(pane))
            .is_some_and(|bytes| {
                bytes
                    .windows(b"FLOOD_DONE".len())
                    .any(|w| w == b"FLOOD_DONE")
            });
        if saw_last_frame {
            let seen_at = last_frame_seen_at.get_or_insert_with(Instant::now);
            if seen_at.elapsed() >= Duration::from_millis(500) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    assert_eq!(
        snapshots, 0,
        "an open attach Surface must not pause/recapture after output loss (events={total_events}, outputs={total_outputs}, bytes={total_output_bytes})"
    );
    assert!(
        (1..=400).contains(&total_outputs),
        "the 5000-frame burst must stay coalesced and bounded (events={total_events}, outputs={total_outputs}, bytes={total_output_bytes})"
    );
    let tail = model
        .state()
        .pane_output(&muxterm::core::types::PaneId(pane))
        .map(|bytes| {
            String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(240)..]).into_owned()
        })
        .unwrap_or_default();
    assert!(
        saw_last_frame,
        "live recovery must contain the last flood frame; tail={tail:?}"
    );
    let _ = runtime.block_on(model.shutdown());
}
