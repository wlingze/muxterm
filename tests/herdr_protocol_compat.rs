//! Real named-session compatibility checks for Herdr client-socket formats.
//! Set MUXTERM_TEST_HERDR_V20_BINARY and MUXTERM_TEST_HERDR_V22_BINARY
//! to the matching official binaries to exercise newer formats.

mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use muxterm::test_support::core::catalog::Catalog;
use muxterm::test_support::core::muxterm::Muxterm;
use muxterm::test_support::core::protocol::task::{Task, TaskOutcome};
use muxterm::test_support::core::protocol::PaneId;
use muxterm::test_support::core::runtime::herdr::observe::{
    channel, ObserveStream, PaneStreamEvent, StreamMode,
};
use muxterm::test_support::core::runtime::herdr::session::HerdrSession;
use muxterm::test_support::core::transport::registry::ConnectionRegistry;
use muxterm::test_support::core::workspace::{WorkspacePool, WorkspaceSpec};
use support::herdr_test_support::IsolatedHerdr;

#[test]
fn v20_named_session_streams_real_pane_frames() -> Result<()> {
    let Some(binary) = std::env::var_os("MUXTERM_TEST_HERDR_V20_BINARY") else {
        return Ok(());
    };
    let binary = PathBuf::from(binary);
    let herdr = IsolatedHerdr::start_with_binary("protocol-v20", &binary);
    let (_workspace, _tab, target) = herdr.create_workspace("/tmp", "protocol-v20");
    let session = Arc::new(HerdrSession::new(herdr.name(), herdr.socket_path()));
    let snapshot = session.snapshot()?;
    ensure!(
        snapshot.protocol == 20,
        "expected protocol 20, got {}",
        snapshot.protocol
    );
    let (tx, rx) = channel();
    let _stream = ObserveStream::start_with_session(
        session,
        &target,
        PaneId(1),
        1,
        StreamMode::Control,
        false,
        80,
        24,
        tx,
    )
    .context("connect v20 control stream")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(PaneStreamEvent::Frame { full: true, .. }) =
            rx.recv_timeout(Duration::from_millis(100))
        {
            return Ok(());
        }
    }
    anyhow::bail!("v20 pane did not deliver a full frame")
}

#[test]
fn v22_named_session_streams_real_pane_frames() -> Result<()> {
    let Some(binary) = std::env::var_os("MUXTERM_TEST_HERDR_V22_BINARY") else {
        return Ok(());
    };
    let binary = PathBuf::from(binary);
    let herdr = IsolatedHerdr::start_with_binary("protocol-v22", &binary);
    let (_workspace, _tab, target) = herdr.create_workspace("/tmp", "protocol-v22");
    herdr.wait_for_pane_process(&target);
    let session = Arc::new(HerdrSession::new(herdr.name(), herdr.socket_path()));
    let snapshot = session.snapshot()?;
    ensure!(
        snapshot.protocol == 22,
        "expected protocol 22, got {}",
        snapshot.protocol
    );
    let (tx, rx) = channel();
    let mut stream = ObserveStream::start_with_session(
        session,
        &target,
        PaneId(1),
        1,
        StreamMode::Control,
        false,
        80,
        24,
        tx,
    )
    .context("connect v22 control stream")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_full = false;
    while Instant::now() < deadline {
        if let Ok(PaneStreamEvent::Frame { full: true, .. }) =
            rx.recv_timeout(Duration::from_millis(100))
        {
            saw_full = true;
            break;
        }
    }
    ensure!(saw_full, "v22 pane did not deliver a full frame");

    // A real child requests mouse reporting, then checks press, hover and
    // wheel bytes. The server must publish mode changes to this direct client.
    let script = herdr.socket_path().with_file_name("muxterm-mouse-probe.py");
    std::fs::write(
        &script,
        r#"import os, select, sys, termios, time
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
mode = termios.tcgetattr(fd)
mode[3] &= ~(termios.ECHO | termios.ICANON)
termios.tcsetattr(fd, termios.TCSANOW, mode)
try:
    os.write(1, b'\x1b[?1003h\x1b[?1006hMOUSE_READY\r\n')
    received = bytearray()
    deadline = time.monotonic() + 8
    expected = (b'\x1b[<0;4;10M', b'\x1b[<35;5;11M', b'\x1b[<64;38;13M')
    while time.monotonic() < deadline:
        if select.select([fd], [], [], 0.1)[0]:
            received.extend(os.read(fd, 128))
            if all(item in received for item in expected):
                os.write(1, b'MOUSE_OK\r\n')
                break
finally:
    os.write(1, b'\x1b[?1003l\x1b[?1006l')
    termios.tcsetattr(fd, termios.TCSANOW, old)
"#,
    )?;
    let command = format!("python3 {}\r", script.display());
    stream.send_input(command.as_bytes())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut captured = false;
    while Instant::now() < deadline {
        if let Ok(PaneStreamEvent::MouseCapture { enabled: true, .. }) =
            rx.recv_timeout(Duration::from_millis(100))
        {
            captured = true;
            break;
        }
    }
    ensure!(captured, "v22 direct attach did not publish mouse capture");
    stream.send_input(b"\x1b[<0;4;10M\x1b[<35;5;11M")?;
    stream.scroll_at(1, Some((37, 12)), 0)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(PaneStreamEvent::Frame { bytes, .. }) =
            rx.recv_timeout(Duration::from_millis(100))
        {
            if bytes
                .windows(b"MOUSE_OK".len())
                .any(|part| part == b"MOUSE_OK")
            {
                return Ok(());
            }
        }
    }
    anyhow::bail!("v22 child did not receive click, hover and wheel")
}

#[test]
fn v19_tab_switch_accepts_input_before_control_stream_is_ready() -> Result<()> {
    tab_switch_accepts_immediate_input(IsolatedHerdr::start("switch-input-v19"), 19)
}

#[test]
fn v20_tab_switch_accepts_input_before_control_stream_is_ready() -> Result<()> {
    let Some(binary) = std::env::var_os("MUXTERM_TEST_HERDR_V20_BINARY") else {
        return Ok(());
    };
    tab_switch_accepts_immediate_input(
        IsolatedHerdr::start_with_binary("switch-input-v20", &PathBuf::from(binary)),
        20,
    )
}

#[test]
fn v22_tab_switch_accepts_input_before_control_stream_is_ready() -> Result<()> {
    let Some(binary) = std::env::var_os("MUXTERM_TEST_HERDR_V22_BINARY") else {
        return Ok(());
    };
    tab_switch_accepts_immediate_input(
        IsolatedHerdr::start_with_binary("switch-input-v22", &PathBuf::from(binary)),
        22,
    )
}

fn tab_switch_accepts_immediate_input(herdr: IsolatedHerdr, expected_protocol: u64) -> Result<()> {
    let executor = tokio::runtime::Runtime::new()?;
    let _entered = executor.enter();
    let (workspace_id, _first_tab, first_wire) = herdr.create_workspace("/tmp", "switch-input");
    let session = HerdrSession::new(herdr.name(), herdr.socket_path());
    ensure!(
        session.snapshot()?.protocol == expected_protocol,
        "unexpected Herdr protocol"
    );
    let created = herdr
        .cli()
        .args([
            "tab",
            "create",
            "--workspace",
            &workspace_id,
            "--label",
            "second",
        ])
        .output()?;
    ensure!(
        created.status.success(),
        "tab create: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    herdr.wait_for_pane_process(&first_wire);

    let spec = WorkspaceSpec::herdr(
        herdr.name(),
        &workspace_id,
        herdr.socket_path().to_string_lossy(),
    );
    let catalog = Catalog::with_builtins();
    let mut connections = ConnectionRegistry::new();
    let mut pool = WorkspacePool::default();
    let runtime = Muxterm::new_runtime(&catalog, &mut connections, &spec)?;
    let workspace = executor.block_on(pool.open_spec_with_runtime(&spec, runtime))?;
    ensure!(workspace.state().tabs().len() == 2, "expected two tabs");
    let target_tab = workspace
        .state()
        .tabs()
        .iter()
        .find(|tab| !tab.active)
        .context("missing hidden tab")?
        .id;
    let target_pane = workspace
        .state()
        .panes(&target_tab)
        .first()
        .context("missing target pane")?
        .id;
    ensure!(
        workspace.execute(Task::ResizeClient {
            cols: 100,
            rows: 30
        })? == TaskOutcome::Done,
        "client resize failed"
    );
    ensure!(
        workspace.execute(Task::ResizePane {
            target: target_pane,
            cols: 100,
            rows: 30
        })? == TaskOutcome::Done,
        "pane resize failed"
    );
    ensure!(
        workspace.execute(Task::SwitchTab { target: target_tab })? == TaskOutcome::Done,
        "tab switch failed"
    );
    let output_path =
        std::env::temp_dir().join(format!("muxterm-test-herdr-switch-input-{}", herdr.name()));
    let command = format!("printf EARLY_OK > {}\r", output_path.display());
    ensure!(
        workspace.execute(Task::WriteRaw {
            target: target_pane,
            data: command.into_bytes()
        })? == TaskOutcome::Done,
        "immediate input failed"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let _ = workspace.refresh();
        if std::fs::read_to_string(&output_path).is_ok_and(|text| text == "EARLY_OK") {
            let _ = std::fs::remove_file(&output_path);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    anyhow::bail!(
        "input sent directly after protocol {expected_protocol} tab switch never reached the shell"
    )
}
