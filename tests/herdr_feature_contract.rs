//! H2：Herdr attach 契约：夹具先涂 `HERDR_LIVE_*`，`Workspace::new` + `connect`
//! 后 `search_workspace` 非空。禁止 MockRuntime 喂字节。

mod support;

use std::sync::Arc;
use std::time::Instant;

use muxterm::core::attention::signal::AttentionSignal;
use muxterm::core::attention::state::PaneStatus;
use muxterm::core::model::layout::{LayoutNode, SplitDir};
use muxterm::core::model::state::{PaneAgentSessionKind, PaneAgentStatus, StateChange};
use muxterm::core::model::task::Task;
use muxterm::core::runtime::herdr::session::{HerdrAgentStatus, HerdrSession};
use muxterm::core::runtime::HerdrRuntime;
use muxterm::core::workspace::id::WorkspaceId;
use muxterm::core::workspace::workspace::Workspace;
use support::herdr_test_support::{herdr_available, IsolatedHerdr, TempAgentCommand};

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
        herdr.paint_until_token(pane, token);
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
    match &workspace
        .state()
        .layout(&tab)
        .expect("三 pane tab 必须有 layout")
        .tree
    {
        LayoutNode::Split {
            dir: SplitDir::Horizontal,
            first,
            second,
            ..
        } => {
            assert!(
                matches!(
                    first.as_ref(),
                    LayoutNode::Split {
                        dir: SplitDir::Vertical,
                        ..
                    }
                ),
                "pP 先向右、再向下分割应保留 H(V(pP,pR),pQ)，实际 first={first:?}"
            );
            assert!(
                matches!(second.as_ref(), LayoutNode::Leaf(_)),
                "右侧 pQ 应是 leaf，实际 second={second:?}"
            );
        }
        tree => panic!("right + down 应还原嵌套 H(V(...),...)，实际 {tree:?}"),
    }

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

/// 两 pane `down` 是用户本次回归的最小复现：snapshot 的 split 方向、ratio、
/// 每个 pane rect 和输出归属都必须完整进入 Muxterm 产品模型。
#[test]
fn herdr_down_split_preserves_vertical_tree_rects_and_output_isolation() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }

    let herdr = IsolatedHerdr::start("layout-down");
    let (ws, herdr_tab, top_pane) = herdr.create_workspace("/tmp", "mux-layout-down");
    let bottom_pane = herdr.split_pane(&top_pane, "down");
    let top_token = "HERDR_LAYOUT_TOP_ONLY";
    let bottom_token = "HERDR_LAYOUT_BOTTOM_ONLY";
    // 新 pane 的 shell 可能在 split 返回后尚未 ready；等待服务端 recent
    // snapshot 确认 token，避免本地快速 runner 与 CI 慢 runner 的时序差异。
    herdr.paint_until_token(&top_pane, top_token);
    herdr.paint_until_token(&bottom_pane, bottom_token);

    let session = Arc::new(HerdrSession::new(herdr.name(), herdr.socket_path()));
    let snapshot = session.snapshot().expect("应读取真实 Herdr snapshot");
    let source_layout = snapshot
        .layouts
        .iter()
        .find(|layout| layout.tab_id == herdr_tab)
        .expect("snapshot 应含 down split layout");
    assert_eq!(source_layout.panes.len(), 2);

    let runtime = HerdrRuntime::new(Arc::clone(&session), &ws);
    let mut workspace = Workspace::new(
        WorkspaceId::new("local", None, herdr.name(), "herdr", &ws),
        "herdr-layout-down".to_string(),
        Box::new(runtime),
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(workspace.connect())
        .expect("Herdr down split attach 应成功");

    let tab = workspace.state().tabs()[0].id;
    let (top, bottom, ratio) = match &workspace
        .state()
        .layout(&tab)
        .expect("down split 必须有 layout")
        .tree
    {
        LayoutNode::Split {
            dir: SplitDir::Vertical,
            ratio,
            first,
            second,
        } => match (first.as_ref(), second.as_ref()) {
            (LayoutNode::Leaf(top), LayoutNode::Leaf(bottom)) => (*top, *bottom, *ratio),
            pair => panic!("两 pane down split 的两个 child 都应是 leaf，实际 {pair:?}"),
        },
        tree => panic!("Herdr down split 必须还原为 Vertical，实际 {tree:?}"),
    };
    assert!(
        (450..=550).contains(&ratio),
        "默认 down split ratio 应接近一半，实际 {ratio}"
    );

    let pane_infos = workspace.state().panes(&tab);
    let top_info = pane_infos
        .iter()
        .find(|pane| pane.id == top)
        .expect("缺上 pane");
    let bottom_info = pane_infos
        .iter()
        .find(|pane| pane.id == bottom)
        .expect("缺下 pane");
    assert!(
        top_info.rows < source_layout.area.height && bottom_info.rows < source_layout.area.height,
        "上下 pane 必须使用各自 rect 高度，不能都继承 tab 整体 {} 行；实际 top={} bottom={}",
        source_layout.area.height,
        top_info.rows,
        bottom_info.rows
    );
    assert!(
        top_info.cols <= source_layout.area.width && bottom_info.cols <= source_layout.area.width,
        "pane cols 不得超过 layout 整体宽度 {}；实际 top={} bottom={}",
        source_layout.area.width,
        top_info.cols,
        bottom_info.cols
    );
    let mut expected_sizes = source_layout
        .panes
        .iter()
        .map(|pane| (pane.rect.width, pane.rect.height))
        .collect::<Vec<_>>();
    expected_sizes.sort_unstable();
    let mut actual_sizes = pane_infos
        .iter()
        .map(|pane| (pane.cols, pane.rows))
        .collect::<Vec<_>>();
    actual_sizes.sort_unstable();
    assert_eq!(
        actual_sizes, expected_sizes,
        "PaneInfo cols/rows 必须逐 pane 等于 Herdr panes[].rect"
    );

    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        let _ = workspace.refresh();
        let top_output = workspace.state().pane_output(&top).unwrap_or_default();
        let bottom_output = workspace.state().pane_output(&bottom).unwrap_or_default();
        if String::from_utf8_lossy(top_output).contains(top_token)
            && String::from_utf8_lossy(bottom_output).contains(bottom_token)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let top_output =
        String::from_utf8_lossy(workspace.state().pane_output(&top).unwrap_or_default());
    let bottom_output =
        String::from_utf8_lossy(workspace.state().pane_output(&bottom).unwrap_or_default());
    assert!(top_output.contains(top_token), "上 pane 缺自己的 token");
    assert!(
        !top_output.contains(bottom_token),
        "下 pane 数据不得串到上 pane: {top_output:?}"
    );
    assert!(
        bottom_output.contains(bottom_token),
        "下 pane 缺自己的 token"
    );
    assert!(
        !bottom_output.contains(top_token),
        "上 pane 数据不得串到下 pane: {bottom_output:?}"
    );

    rt.block_on(workspace.shutdown())
        .expect("Herdr layout contract shutdown 应成功");
}

/// Muxterm 自己发起的 down split 不能只把请求参数发对；响应落入本地状态时
/// 也必须保留 Vertical。旧实现请求 `down` 后仍硬编码 Horizontal。
#[test]
fn herdr_runtime_split_pane_down_updates_vertical_layout() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }

    let herdr = IsolatedHerdr::start("task-split-down");
    let (ws, _tab, _pane) = herdr.create_workspace("/tmp", "mux-task-split-down");
    let session = Arc::new(HerdrSession::new(herdr.name(), herdr.socket_path()));
    let runtime = HerdrRuntime::new(session, &ws);
    let mut workspace = Workspace::new(
        WorkspaceId::new("local", None, herdr.name(), "herdr", &ws),
        "herdr-task-split-down".to_string(),
        Box::new(runtime),
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(workspace.connect())
        .expect("Herdr 单 pane attach 应成功");

    let target = workspace
        .state()
        .active_pane()
        .expect("应有 active pane")
        .id;
    // W5：mutation 异步收敛（Accepted → MutationSettled），不能同步断言拓扑。
    let outcome = workspace
        .execute(Task::SplitPane {
            target: Some(target),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        })
        .expect("Herdr down SplitPane 应成功");
    let operation_id = match outcome {
        muxterm::core::model::task::TaskOutcome::Accepted { operation_id } => operation_id,
        other => panic!("SplitPane 必须 Accepted，实际 {other:?}"),
    };

    // 等唯一 MutationSettled(Completed) + 快照收敛出 Vertical 布局。
    let deadline = Instant::now() + HERDR_TIMEOUT;
    let mut settled = false;
    while Instant::now() < deadline {
        for event in workspace.take_events() {
            if let StateChange::MutationSettled {
                operation_id: settled_id,
                result,
                ..
            } = event
            {
                assert_eq!(settled_id, operation_id, "settlement operation 不匹配");
                assert_eq!(
                    result,
                    muxterm::core::model::state::MutationResult::Completed,
                    "SplitPane 必须 Completed"
                );
                settled = true;
            }
        }
        let _ = workspace.refresh();
        let tab = workspace.state().tabs()[0].id;
        if settled
            && matches!(
                workspace.state().layout(&tab).map(|layout| &layout.tree),
                Some(LayoutNode::Split {
                    dir: SplitDir::Vertical,
                    ..
                })
            )
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    let tab = workspace.state().tabs()[0].id;
    assert!(
        matches!(
            workspace.state().layout(&tab).map(|layout| &layout.tree),
            Some(LayoutNode::Split {
                dir: SplitDir::Vertical,
                ..
            })
        ),
        "Task::SplitPane down 后本地 LayoutNode 也必须是 Vertical，实际 {:?}",
        workspace.state().layout(&tab).map(|layout| &layout.tree)
    );

    rt.block_on(workspace.shutdown())
        .expect("Herdr task split shutdown 应成功");
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
    let mut live_frame_pane = None;
    while Instant::now() < ready_deadline {
        let events = workspace.refresh();
        live_frame_pane = events
            .iter()
            .find_map(|event| match event {
                StateChange::PaneFrame { pane, .. } => Some(*pane),
                _ => None,
            })
            .or(live_frame_pane);
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
    // `pane.read`/Index seed can satisfy the ready-token assertion before the
    // asynchronous control stream has completed its first full frame.  Wait
    // for that real stream event so the following VTE-style per-byte commits
    // exercise a live control path rather than racing startup on slow CI.
    let mut saw_live_frame = live_frame_pane == Some(active);
    if !saw_live_frame {
        let frame_deadline = Instant::now() + HERDR_TIMEOUT;
        while Instant::now() < frame_deadline {
            let events = workspace.refresh();
            if events.iter().any(|event| {
                matches!(
                    event,
                    StateChange::PaneFrame { pane, .. } if *pane == active
                )
            }) {
                saw_live_frame = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    assert!(
        saw_live_frame,
        "本地 Herdr active pane 必须先收到 control stream full frame"
    );
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

/// Herdr 的 agent snapshot 与订阅事件必须先在 Runtime 层翻译成统一的
/// PaneAgent/Attention 语义，再由 Workspace 缓存和转交；上层不能解析
/// `pane.agent_status_changed` 或 Herdr public pane id。
#[test]
fn herdr_agent_snapshot_and_transitions_reach_workspace_with_full_metadata() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }

    let agent_command = TempAgentCommand::pi("events");
    let herdr = IsolatedHerdr::start("agent-events");
    let (ws, _tab, herdr_pane) = herdr.create_workspace(
        agent_command
            .cwd()
            .to_str()
            .expect("临时 agent cwd 不是 UTF-8"),
        "mux-agent-events",
    );
    let session = Arc::new(HerdrSession::new(herdr.name(), herdr.socket_path()));
    // `herdr:pi` is a protocol-defined full-lifecycle authority. Sources such
    // as `herdr:codex` report session identity only and therefore cannot drive
    // this real working -> blocked transition fixture.
    let source = "herdr:pi";
    let agent_session_path = agent_command.cwd().join("pi-session.jsonl");
    std::fs::write(&agent_session_path, "{}").expect("创建临时 pi session 失败");

    herdr.paint(&herdr_pane, agent_command.invocation());
    let deadline = Instant::now() + HERDR_TIMEOUT;
    loop {
        let snapshot = session
            .snapshot()
            .expect("等待 pi detection 时读取 snapshot 失败");
        if snapshot
            .agents
            .iter()
            .any(|agent| agent.pane_id == herdr_pane && agent.agent.as_deref() == Some("pi"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Herdr 未识别隔离 pane 里的真实 pi agent: {snapshot:#?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    session
        .call(
            "pane.report_agent_session",
            serde_json::json!({
                "pane_id": herdr_pane,
                "source": source,
                "agent": "pi",
                "seq": 1,
                "agent_session_path": agent_session_path,
                "session_start_source": "startup",
            }),
        )
        .expect("应能报告完整 pi agent session 引用");
    session
        .call(
            "pane.report_agent",
            serde_json::json!({
                "pane_id": herdr_pane,
                "source": source,
                "agent": "pi",
                "state": "working",
                "message": "implementing runtime events",
                "seq": 2,
                "agent_session_path": agent_session_path,
            }),
        )
        .expect("应能在隔离 Herdr pane 上报告 working agent");
    session
        .call(
            "pane.report_metadata",
            serde_json::json!({
                "pane_id": herdr_pane,
                "source": "muxterm-test:display",
                "agent": "pi",
                "applies_to_source": source,
                "title": "Implement runtime events",
                "display_agent": "Pi reviewer",
                "state_labels": {
                    "working": "Implementing",
                    "blocked": "Needs approval"
                },
                "tokens": {
                    "phase": "tests",
                    "context": "73%"
                },
                "seq": 1,
            }),
        )
        .expect("应能报告完整 agent display metadata");

    let runtime = HerdrRuntime::new(Arc::clone(&session), &ws);
    let mut workspace = Workspace::new(
        WorkspaceId::new("local", None, herdr.name(), "herdr", &ws),
        "herdr-agent-events".to_string(),
        Box::new(runtime),
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(workspace.connect())
        .expect("Herdr agent Runtime connect 应成功");

    let pane = workspace
        .state()
        .active_pane()
        .expect("agent workspace 应有 active pane")
        .id;
    let initial_events = workspace.take_events();
    assert!(
        initial_events.iter().any(|event| matches!(
            event,
            StateChange::PaneAgentChanged {
                pane: event_pane,
                agent: Some(agent),
                initial: true,
            } if *event_pane == pane && agent.status == PaneAgentStatus::Working
        )),
        "initial agent event missing: {initial_events:#?}"
    );
    let agent = workspace
        .pane_agent(pane)
        .expect("Workspace 应缓存 agent 快照");
    assert_eq!(agent.kind.as_deref(), Some("pi"));
    assert_eq!(agent.title.as_deref(), Some("Implement runtime events"));
    assert_eq!(agent.display_name.as_deref(), Some("Pi reviewer"));
    assert_eq!(
        agent.state_labels.get("blocked").map(String::as_str),
        Some("Needs approval")
    );
    assert_eq!(agent.tokens.get("phase").map(String::as_str), Some("tests"));
    let agent_session = agent
        .session
        .as_ref()
        .expect("完整快照应保留 agent session");
    assert_eq!(agent_session.kind, PaneAgentSessionKind::Path);
    assert_eq!(agent_session.value, agent_session_path.to_string_lossy());
    assert!(workspace.take_attention_signals(pane).iter().any(|signal| {
        matches!(
            signal,
            AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Working,
                initial: true,
            }
        )
    }));

    session
        .call(
            "pane.report_agent",
            serde_json::json!({
                "pane_id": herdr_pane,
                "source": source,
                "agent": "pi",
                "state": "blocked",
                "message": "approve the command",
                "seq": 3,
                "agent_session_path": agent_session_path,
            }),
        )
        .expect("应能报告 blocked transition");
    let direct_snapshot = session
        .snapshot()
        .expect("blocked 后应能读取 Herdr snapshot");
    assert!(
        direct_snapshot.agents.iter().any(|agent| {
            agent.pane_id == herdr_pane && agent.agent_status == HerdrAgentStatus::Blocked
        }),
        "Herdr fixture itself did not reach blocked: {direct_snapshot:#?}"
    );

    let deadline = Instant::now() + HERDR_TIMEOUT;
    let mut blocked_event = false;
    while Instant::now() < deadline {
        let events = workspace.refresh();
        blocked_event |= events.iter().any(|event| {
            matches!(
                event,
                StateChange::PaneAgentChanged {
                    pane: event_pane,
                    agent: Some(agent),
                    initial: false,
                } if *event_pane == pane && agent.status == PaneAgentStatus::Blocked
            )
        });
        if blocked_event {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        blocked_event,
        "真实 Herdr blocked 事件必须进入统一 StateChange"
    );
    assert_eq!(
        workspace.pane_agent(pane).map(|agent| agent.status),
        Some(PaneAgentStatus::Blocked)
    );
    assert!(workspace.take_attention_signals(pane).iter().any(|signal| {
        matches!(
            signal,
            AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Blocked,
                initial: false,
            }
        )
    }));

    session
        .call(
            "pane.release_agent",
            serde_json::json!({
                "pane_id": herdr_pane,
                "source": source,
                "agent": "pi",
                "seq": 4,
            }),
        )
        .expect("应能释放测试 agent authority");
    session
        .pane_send_keys(&herdr_pane, &["ctrl+c".to_string()])
        .expect("应能停止隔离 pi agent");
    let deadline = Instant::now() + HERDR_TIMEOUT;
    let mut released = false;
    while Instant::now() < deadline {
        let events = workspace.refresh();
        released |= events.iter().any(|event| {
            matches!(
                event,
                StateChange::PaneAgentChanged {
                    pane: event_pane,
                    agent: None,
                    initial: false,
                } if *event_pane == pane
            )
        });
        if released {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(released, "agent release 必须清除 Workspace 的 pane agent");
    assert!(workspace.pane_agent(pane).is_none());
    assert!(workspace
        .take_attention_signals(pane)
        .iter()
        .any(|signal| matches!(signal, AttentionSignal::ClearAuthoritativeStatus)));

    rt.block_on(workspace.shutdown())
        .expect("Herdr agent contract shutdown 应成功");
}
