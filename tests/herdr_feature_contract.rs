//! H2：Herdr attach 契约：夹具先涂 `HERDR_LIVE_*`，`Workspace::new` + `connect`
//! 后 `search_workspace` 非空。禁止 MockRuntime 喂字节。

mod support;

use std::sync::Arc;
use std::time::Instant;

use muxterm::core::model::layout::{LayoutNode, SplitDir};
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
    herdr.paint(&top_pane, top_token);
    herdr.paint(&bottom_pane, bottom_token);

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
    workspace
        .execute(Task::SplitPane {
            target: Some(target),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        })
        .expect("Herdr down SplitPane 应成功");

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
