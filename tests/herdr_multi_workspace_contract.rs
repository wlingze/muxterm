//! H3：同一 IsolatedHerdr 上两格 Workspace，共享**一条** HerdrSession。
//!
//! 两个 token 各自命中自己的工作区，互不污染；内部必须是同一 socket
//! （Arc::ptr_eq），禁止 connect 两次各建无关 client 假装共享。

mod support;

use std::sync::Arc;
use std::time::Instant;

use muxterm::core::runtime::HerdrRuntime;
use muxterm::core::workspace::pool::{WorkspacePool, WorkspacePoolPolicy};
use muxterm::core::workspace::spec::WorkspaceSpec;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};

const HERDR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// 同一 socket 两格：各自 token 只出现在自己的工作区。
#[test]
fn herdr_multi_workspace_contract() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let herdr = IsolatedHerdr::start("multi");
    let (ws_a, _ta, pane_a) = herdr.create_workspace("/tmp", "mux-a");
    let (ws_b, _tb, pane_b) = herdr.create_workspace("/tmp", "mux-b");
    let token_a = format!("HERDR_LIVE_{}", "multi-a");
    let token_b = format!("HERDR_LIVE_{}", "multi-b");
    herdr.paint(&pane_a, &token_a);
    herdr.paint(&pane_b, &token_b);

    let spec_a = WorkspaceSpec::herdr(
        herdr.name(),
        ws_a.clone(),
        herdr.socket_path().to_string_lossy().to_string(),
    );
    let spec_b = WorkspaceSpec::herdr(
        herdr.name(),
        ws_b.clone(),
        herdr.socket_path().to_string_lossy().to_string(),
    );
    let id_a = spec_a.id();
    let id_b = spec_b.id();

    let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(pool.open_spec(&spec_a)).expect("open A 失败");
    rt.block_on(pool.open_spec(&spec_b)).expect("open B 失败");

    // 内部必须共享同一条 HerdrSession（同一 socket）。
    let sa = pool
        .get(&id_a)
        .unwrap()
        .runtime()
        .as_any()
        .downcast_ref::<HerdrRuntime>()
        .expect("A 应是 HerdrRuntime")
        .session_arc()
        .clone();
    let sb = pool
        .get(&id_b)
        .unwrap()
        .runtime()
        .as_any()
        .downcast_ref::<HerdrRuntime>()
        .expect("B 应是 HerdrRuntime")
        .session_arc()
        .clone();
    assert!(
        Arc::ptr_eq(&sa, &sb),
        "同一 socket 两格必须共享一条 HerdrSession（禁止各建无关 client）"
    );

    let deadline = Instant::now() + HERDR_TIMEOUT;
    let mut ok_a = false;
    let mut ok_b = false;
    while Instant::now() < deadline {
        let _ = pool.get_mut(&id_a).unwrap().refresh();
        let _ = pool.get_mut(&id_b).unwrap().refresh();
        let hits_a = pool.get(&id_a).unwrap().search_workspace(&token_a);
        let hits_b = pool.get(&id_b).unwrap().search_workspace(&token_b);
        ok_a = !hits_a.is_empty();
        ok_b = !hits_b.is_empty();
        if ok_a && ok_b {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(ok_a, "A 必须命中自己的 token {token_a}");
    assert!(ok_b, "B 必须命中自己的 token {token_b}");
    assert!(
        pool.get(&id_a)
            .unwrap()
            .search_workspace(&token_b)
            .is_empty(),
        "A 不得污染 B 的 token"
    );
    assert!(
        pool.get(&id_b)
            .unwrap()
            .search_workspace(&token_a)
            .is_empty(),
        "B 不得污染 A 的 token"
    );
    pool.shutdown_all();
}
