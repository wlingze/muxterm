//! H1：HerdrSession 连**隔离 named session**，snapshot 能看到刚 create 的 workspace。
//!
//! 无 herdr 二进制才 skip；有二进制不许 ignore。禁止连用户默认 herdr.sock。

mod support;

use muxterm::core::runtime::herdr::session::HerdrSession;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};

/// 夹具先 create + paint，再 `HerdrSession::connect` snapshot：
/// 必须看到刚 create 的 workspace_id，pane 拓扑非空，且 socket 不是用户默认。
#[test]
fn herdr_named_session_snapshot_sees_painted_workspace() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let herdr = IsolatedHerdr::start("snap");
    assert_ne!(
        herdr.socket_path(),
        std::path::Path::new("/home/wlz/.config/herdr/herdr.sock"),
        "测试必须连隔离 named session，禁止用户默认 herdr.sock"
    );

    let (ws, _tab, pane) = herdr.create_workspace("/tmp", "mux-h1");
    let token = format!("HERDR_LIVE_{}", "snap");
    herdr.paint(&pane, &token);

    let session = HerdrSession::new(herdr.name(), herdr.socket_path());
    session.ping().expect("ping 应成功");
    let snap = session.snapshot().expect("session.snapshot 应成功");

    assert!(
        snap.workspaces.iter().any(|w| w.workspace_id == ws),
        "snapshot 必须看到刚 create 的 workspace {ws}: {:?}",
        snap.workspaces
    );
    assert!(
        !snap.panes.is_empty(),
        "snapshot 的 pane 拓扑非空: {:?}",
        snap.panes
    );
    assert!(
        snap.panes.iter().any(|p| p.pane_id == pane),
        "snapshot 必须包含 pane {pane}"
    );
}
