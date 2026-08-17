//! H2：Herdr attach 契约：夹具先涂 `HERDR_LIVE_*`，`Workspace::new` + `connect`
//! 后 `search_workspace` 非空。禁止 MockRuntime 喂字节。

mod support;

use std::sync::Arc;
use std::time::Instant;

use muxterm::core::runtime::herdr::session::HerdrSession;
use muxterm::core::runtime::HerdrRuntime;
use muxterm::core::workspace::id::WorkspaceId;
use muxterm::core::workspace::workspace::Workspace;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};

/// 与 SSH 契约同量级（15s）。
const HERDR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// 夹具先涂 token 再 attach：PaneBuf 必须能搜到（直播走 observe 流，
/// 不是 MockRuntime 喂字节）。
#[test]
fn herdr_attach_preexist_token_reaches_workspace() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let herdr = IsolatedHerdr::start("feat");
    let (ws, _tab, pane) = herdr.create_workspace("/tmp", "mux-h2");
    let token = format!("HERDR_LIVE_{}", "feat");
    herdr.paint(&pane, &token);

    let session = Arc::new(HerdrSession::new(herdr.name(), herdr.socket_path()));
    let runtime = HerdrRuntime::new(session, &ws);
    let mut workspace = Workspace::new(
        WorkspaceId::new("local", None, herdr.name(), "herdr", &ws),
        herdr.name().to_string(),
        Box::new(runtime),
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(workspace.connect())
        .expect("Herdr attach 失败（隔离 named session）");

    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        let _ = workspace.refresh();
        if !workspace.search_workspace(&token).is_empty() {
            let _ = rt.block_on(workspace.shutdown());
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let hits = workspace.search_workspace(&token).len();
    let _ = rt.block_on(workspace.shutdown());
    panic!(
        "Herdr attach 后 PaneBuf 必须含播种 token {token}。禁止 MockRuntime 喂字节。hits={hits}"
    );
}
