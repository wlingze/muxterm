//! H2：Herdr attach 契约：夹具先涂 `HERDR_LIVE_*`，`Workspace::new` + `connect`
//! 后 `search_workspace` 非空。禁止 MockRuntime 喂字节。

mod support;

use std::sync::Arc;
use std::time::Instant;

use muxterm::core::model::task::Task;
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
    // 推进到真实 dogfood 已出现的 pP/pQ/pR，再在同一 tab 造三 pane。
    // 旧实现把所有字母后缀都映射成 PaneId(0)，导致 L0:L0 与 GTK critical。
    let (ws, _tab, [pane_p, pane_q, pane_r]) = herdr.create_alpha_split_workspace("/tmp", "mux-h2");
    let seed_tokens = [
        "HERDR_LIVE_feat_p",
        "HERDR_LIVE_feat_q",
        "HERDR_LIVE_feat_r",
    ];
    for (pane, token) in [
        (pane_p.as_str(), seed_tokens[0]),
        (pane_q.as_str(), seed_tokens[1]),
        (pane_r.as_str(), seed_tokens[2]),
    ] {
        herdr.paint(pane, token);
    }

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

    let tab = workspace.state().tabs()[0].id;
    let leaves = workspace
        .state()
        .layout(&tab)
        .expect("三 pane tab 必须有 layout")
        .tree
        .leaves();
    let unique: std::collections::HashSet<_> = leaves.iter().copied().collect();
    assert_eq!(
        leaves.len(),
        3,
        "真实 Herdr layout 必须保留三个 leaf: {leaves:?}"
    );
    assert_eq!(
        unique.len(),
        3,
        "pP/pQ/pR 必须映射到三个唯一产品 PaneId，禁止 L0:L0: {leaves:?}"
    );
    assert!(
        leaves.iter().all(|pane| pane.0 != 0),
        "合法 public id 不得映射成 PaneId(0): {leaves:?}"
    );

    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        let _ = workspace.refresh();
        if seed_tokens
            .iter()
            .all(|token| !workspace.search_workspace(token).is_empty())
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for token in seed_tokens {
        assert!(
            !workspace.search_workspace(token).is_empty(),
            "三个真实 pane 的 attach 快照都必须保留，缺 {token}"
        );
    }
    rt.block_on(workspace.shutdown())
        .expect("Herdr attach contract shutdown 应成功");
}

/// 本地 Herdr 单 pane 输入契约：模拟 VTE 每次 commit 一个字符，最后单独
/// commit Enter。bracketed-paste 开启时，`pane.send_input` 会把 Enter 当作
/// 粘贴内容；只有 WriteRaw 走 `pane.send_text` 才会真的提交命令。
#[test]
fn herdr_local_write_raw_executes_echo_command() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }

    let herdr = IsolatedHerdr::start("input-local");
    let (ws, _tab, pane) = herdr.create_workspace("/tmp", "mux-input-local");
    let ready_command = "echo HERDR_READY_\"LOCAL\"";
    let ready_token = "HERDR_READY_LOCAL";
    assert!(!ready_command.contains(ready_token));
    herdr.paint(&pane, ready_command);

    let session = Arc::new(HerdrSession::new(herdr.name(), herdr.socket_path()));
    let runtime = HerdrRuntime::new(session, &ws);
    let mut workspace = Workspace::new(
        WorkspaceId::new("local", None, herdr.name(), "herdr", &ws),
        "local-herdr-input".to_string(),
        Box::new(runtime),
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(workspace.connect())
        .expect("本地 Herdr Runtime connect 应成功");

    let ready_deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < ready_deadline {
        let _ = workspace.refresh();
        if !workspace.search_workspace(ready_token).is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        !workspace.search_workspace(ready_token).is_empty(),
        "本地 Herdr shell 必须先完成 ready echo"
    );

    let active = workspace
        .state()
        .active_pane()
        .expect("本地 Herdr 应有 active pane")
        .id;
    let command = "echo HERDR_EXEC_\"LOCALCORE\"";
    let output_token = "HERDR_EXEC_LOCALCORE";
    assert!(!command.contains(output_token));
    assert!(workspace.search_workspace(output_token).is_empty());
    for byte in command.bytes().chain(std::iter::once(b'\r')) {
        workspace
            .execute(Task::WriteRaw {
                target: active,
                data: vec![byte],
            })
            .expect("本地 Herdr 逐字 WriteRaw 应成功");
    }

    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        let _ = workspace.refresh();
        if !workspace.search_workspace(output_token).is_empty() {
            let _ = rt.block_on(workspace.shutdown());
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let pane_output = workspace
        .state()
        .pane_output(&active)
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .unwrap_or_default();
    let _ = rt.block_on(workspace.shutdown());
    panic!(
        "本地 Herdr 逐字输入 + Enter 必须执行 echo。token={output_token} output={pane_output:?}"
    );
}
