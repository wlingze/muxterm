//! Herdr 服务端权威焦点与 control/observe pane 绑定契约。
//!
//! local / SSH 都通过生产 Catalog 打开。每次切 tab/pane 后同时比较：
//! Herdr `session.snapshot`、Muxterm Workspace active ids 与 layout active；
//! 每个 pane 的真实命令输出还必须同时出现在服务端 `pane.read` 与 PaneBuf，
//! 并且不能串到其它 pane。

mod support;

use std::collections::HashSet;
use std::ffi::OsString;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};

use muxterm::core::catalog::Catalog;
use muxterm::core::model::layout::SplitDir;
use muxterm::core::model::state::{MutationResult, StateChange};
use muxterm::core::model::task::{Task, TaskOutcome};
use muxterm::core::runtime::HerdrRuntime;
use muxterm::core::types::{PaneId, TabId};
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::core::workspace::workspace::Workspace;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};

const TIMEOUT: Duration = Duration::from_secs(15);

struct EnvRestore {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvRestore {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        if let Some(value) = self.previous.take() {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn wait_until(
    workspace: &mut Workspace,
    label: &str,
    mut predicate: impl FnMut(&Workspace) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        let _ = workspace.refresh();
        if predicate(workspace) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!("等待 {label} 超时")
}

fn done(workspace: &mut Workspace, task: Task, label: &str) -> Result<()> {
    let outcome = workspace.execute(task)?;
    ensure!(
        outcome == TaskOutcome::Done,
        "{label} 必须 Done，实际 {outcome:?}"
    );
    Ok(())
}

/// W5 起 NewTab/SplitPane 是异步 mutation：入队返回 `Accepted { operation_id }`，
/// 最终结果由唯一 `MutationSettled(Completed)` 交付（见 plan §3.1）。
fn accepted(workspace: &mut Workspace, task: Task, label: &str) -> Result<u64> {
    let outcome = workspace.execute(task)?;
    match outcome {
        TaskOutcome::Accepted { operation_id } => Ok(operation_id),
        other => anyhow::bail!("{label} 必须 Accepted，实际 {other:?}"),
    }
}

/// 等待指定 operation 的唯一 `MutationSettled`，且必须 Completed。
/// `refresh()` 会把 runtime 事件洗完並从返回值交付，必须同时检查两路。
fn wait_settled(workspace: &mut Workspace, operation_id: u64, label: &str) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        for event in workspace.take_events() {
            if let Some(()) = check_settled(event, operation_id, label)? {
                return Ok(());
            }
        }
        for event in workspace.refresh() {
            if let Some(()) = check_settled(event, operation_id, label)? {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!("等待 {label} settlement 超时")
}

/// 单条事件是否就是目标 operation 的 Completed settlement。
fn check_settled(event: StateChange, operation_id: u64, label: &str) -> Result<Option<()>> {
    if let StateChange::MutationSettled {
        operation_id: settled,
        result,
        ..
    } = event
    {
        ensure!(
            settled == operation_id,
            "{label} settlement operation {settled} != {operation_id}"
        );
        ensure!(
            result == MutationResult::Completed,
            "{label} 必须 Completed，实际 {result:?}"
        );
        Ok(Some(()))
    } else {
        Ok(None)
    }
}

fn switch_tab(workspace: &mut Workspace, target: TabId, label: &str) -> Result<()> {
    done(workspace, Task::SwitchTab { target }, label)?;
    wait_until(workspace, label, |candidate| {
        candidate.state().active_tab().map(|tab| tab.id) == Some(target)
    })
}

fn switch_pane(workspace: &mut Workspace, target: PaneId, label: &str) -> Result<()> {
    done(workspace, Task::SwitchPane { target }, label)?;
    wait_until(workspace, label, |candidate| {
        candidate.state().active_pane().map(|pane| pane.id) == Some(target)
    })
}

fn active_tab(workspace: &Workspace) -> Result<TabId> {
    workspace
        .state()
        .active_tab()
        .map(|tab| tab.id)
        .context("缺 active tab")
}

fn active_pane(workspace: &Workspace) -> Result<PaneId> {
    workspace
        .state()
        .active_pane()
        .map(|pane| pane.id)
        .context("缺 active pane")
}

fn leaves(workspace: &Workspace, tab: TabId) -> Vec<PaneId> {
    workspace
        .state()
        .layout(&tab)
        .map(|layout| layout.tree.leaves())
        .unwrap_or_default()
}

fn herdr_runtime(workspace: &Workspace) -> Result<&HerdrRuntime> {
    workspace
        .runtime()
        .as_any()
        .downcast_ref::<HerdrRuntime>()
        .context("Catalog 没有打开 HerdrRuntime")
}

fn assert_authoritative_focus(
    workspace: &mut Workspace,
    expected_tab: TabId,
    expected_pane: PaneId,
    label: &str,
) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match check_authoritative_focus(workspace, expected_tab, expected_pane, label) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => {
                // 事件流快照可能晚到（SSH 转发有延迟）：拉一次事件再等下一轮收敛。
                let _ = workspace.refresh();
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn check_authoritative_focus(
    workspace: &Workspace,
    expected_tab: TabId,
    expected_pane: PaneId,
    label: &str,
) -> Result<()> {
    ensure!(
        active_tab(workspace)? == expected_tab,
        "{label}: Workspace active tab 错误"
    );
    ensure!(
        active_pane(workspace)? == expected_pane,
        "{label}: Workspace active pane 错误"
    );
    let layout_state = workspace.state().layout(&expected_tab);
    ensure!(
        layout_state.is_some_and(|layout| layout.active == expected_pane),
        "{label}: Workspace layout.active 错误 active_pane={:?} layout={:?} expected={expected_pane}",
        active_pane(workspace),
        layout_state.map(|l| l.active)
    );

    let runtime = herdr_runtime(workspace)?;
    let wire_tab = runtime
        .test_herdr_tab_id(expected_tab)
        .context("产品 TabId 缺 Herdr wire id")?;
    let wire_pane = runtime
        .test_herdr_pane_id(expected_pane)
        .context("产品 PaneId 缺 Herdr wire id")?;
    let snapshot = runtime.session().snapshot()?;
    let server_workspace = snapshot
        .workspaces
        .iter()
        .find(|candidate| candidate.workspace_id == runtime.workspace_id())
        .context("服务端 snapshot 缺绑定 workspace")?;
    ensure!(
        server_workspace.active_tab_id.as_deref() == Some(wire_tab),
        "{label}: 服务端 workspace.active_tab_id={:?}, expected={wire_tab}",
        server_workspace.active_tab_id
    );
    ensure!(
        snapshot.focused_workspace_id.as_deref() == Some(runtime.workspace_id()),
        "{label}: 服务端 focused_workspace_id={:?}",
        snapshot.focused_workspace_id
    );
    ensure!(
        snapshot.focused_tab_id.as_deref() == Some(wire_tab),
        "{label}: 服务端 focused_tab_id={:?}, expected={wire_tab}",
        snapshot.focused_tab_id
    );
    ensure!(
        snapshot.focused_pane_id.as_deref() == Some(wire_pane),
        "{label}: 服务端 focused_pane_id={:?}, expected={wire_pane}",
        snapshot.focused_pane_id
    );
    let server_layout = snapshot
        .layouts
        .iter()
        .find(|layout| layout.tab_id == wire_tab)
        .context("服务端 snapshot 缺 active tab layout")?;
    ensure!(
        server_layout.focused_pane_id == wire_pane,
        "{label}: 服务端 layout.focused_pane_id={}, expected={wire_pane}",
        server_layout.focused_pane_id
    );
    Ok(())
}

fn execute_and_assert_three_way_binding(
    workspace: &mut Workspace,
    target: PaneId,
    all_panes: &[PaneId],
    suffix: &str,
) -> Result<()> {
    let token = format!("HERDR_AUTH_{suffix}");
    let command = format!("printf 'HERDR_AUTH_%s\\n' '{suffix}'\r");
    ensure!(!command.contains(&token), "输入命令不得原样包含期望 token");
    ensure!(
        workspace.search_workspace(&token).is_empty(),
        "执行前 PaneBuf 不得已有 token"
    );
    let (session, target_wire, other_wires) = {
        let runtime = herdr_runtime(workspace)?;
        let target_wire = runtime
            .test_herdr_pane_id(target)
            .context("target 缺 Herdr wire id")?
            .to_string();
        let other_wires = all_panes
            .iter()
            .filter(|pane| **pane != target)
            .map(|pane| {
                runtime
                    .test_herdr_pane_id(*pane)
                    .with_context(|| format!("pane {pane} 缺 Herdr wire id"))
                    .map(ToOwned::to_owned)
            })
            .collect::<Result<Vec<_>>>()?;
        (runtime.session().clone(), target_wire, other_wires)
    };

    done(
        workspace,
        Task::WriteRaw {
            target,
            data: command.into_bytes(),
        },
        "control stream 执行 printf",
    )?;
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        let _ = workspace.refresh();
        let server_has = session
            .pane_read_ansi(&target_wire)
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(&token));
        let workspace_has = workspace
            .search_pane(target, &token)
            .iter()
            .any(|hit| hit.pane_id == target);
        if server_has && workspace_has {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let server_target = session.pane_read_ansi(&target_wire)?;
    ensure!(
        String::from_utf8_lossy(&server_target).contains(&token),
        "服务端 pane.read({target_wire}) 缺 {token}"
    );
    ensure!(
        workspace
            .search_pane(target, &token)
            .iter()
            .any(|hit| hit.pane_id == target),
        "Workspace PaneBuf target={target} 缺 {token}"
    );
    for (pane, wire) in all_panes
        .iter()
        .copied()
        .filter(|pane| *pane != target)
        .zip(other_wires)
    {
        ensure!(
            !String::from_utf8_lossy(&session.pane_read_ansi(&wire)?).contains(&token),
            "服务端 token {token} 串到 pane {wire}"
        );
        ensure!(
            workspace.search_pane(pane, &token).is_empty(),
            "PaneBuf token {token} 串到产品 pane {pane}"
        );
    }
    Ok(())
}

fn run_case(rt: &tokio::runtime::Runtime, sshd: &LoopbackSshd, transport: &str) -> Result<()> {
    let herdr = IsolatedHerdr::start(&format!("authority-{transport}"));
    let (workspace_id, _tab, _pane) =
        herdr.create_workspace("/tmp", &format!("authority-{transport}"));
    let spec = match transport {
        "local" => WorkspaceSpec::herdr(
            herdr.name(),
            &workspace_id,
            herdr.socket_path().to_string_lossy(),
        ),
        "ssh" => WorkspaceSpec::ssh_herdr(
            sshd.alias.clone(),
            herdr.name(),
            &workspace_id,
            herdr.socket_path().to_string_lossy(),
        ),
        other => anyhow::bail!("未知 transport {other}"),
    };
    let mut catalog = Catalog::with_builtins();
    let workspace = rt.block_on(catalog.open(&spec))?;
    wait_until(workspace, "初始 Herdr tab/pane", |ws| {
        ws.state().tabs().len() == 1 && ws.state().active_pane().is_some()
    })?;

    {
        let runtime = herdr_runtime(workspace)?;
        if transport == "ssh" {
            ensure!(
                runtime.session().socket_path() != herdr.socket_path(),
                "SSH case 必须使用 Runtime 持有的本地 forwarded API socket"
            );
            ensure!(
                runtime
                    .session()
                    .socket_path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("muxterm-herdr-fwd-")),
                "SSH Runtime socket 不是生产 forward: {}",
                runtime.session().socket_path().display()
            );
        } else {
            ensure!(
                runtime.session().socket_path() == herdr.socket_path(),
                "local case 必须直连隔离 named-session socket"
            );
        }
    }

    let tab1 = active_tab(workspace)?;
    let tab1_pane = active_pane(workspace)?;
    assert_authoritative_focus(workspace, tab1, tab1_pane, "初始焦点")?;

    let new_tab_op = accepted(
        workspace,
        Task::NewTab {
            name: Some("authority-tab-2".into()),
            command: None,
            workdir: None,
        },
        "NewTab",
    )?;
    wait_settled(workspace, new_tab_op, "NewTab settlement")?;
    wait_until(workspace, "第二个 tab", |ws| {
        ws.state().tabs().len() == 2
    })?;
    let tab2 = active_tab(workspace)?;
    let left = active_pane(workspace)?;
    assert_authoritative_focus(workspace, tab2, left, "创建 Tab 2 后")?;

    let split_right_op = accepted(
        workspace,
        Task::SplitPane {
            target: Some(left),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        },
        "向右 split",
    )?;
    wait_settled(workspace, split_right_op, "向右 split settlement")?;
    wait_until(workspace, "Tab 2 两 pane", |ws| {
        leaves(ws, tab2).len() == 2
    })?;
    let right_top = active_pane(workspace)?;
    assert_authoritative_focus(workspace, tab2, right_top, "向右 split 后")?;

    let split_down_op = accepted(
        workspace,
        Task::SplitPane {
            target: Some(right_top),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        },
        "向下 split",
    )?;
    wait_settled(workspace, split_down_op, "向下 split settlement")?;
    wait_until(workspace, "Tab 2 三 pane", |ws| {
        leaves(ws, tab2).len() == 3
    })?;
    let tab2_panes = leaves(workspace, tab2);
    ensure!(
        tab2_panes.iter().copied().collect::<HashSet<_>>().len() == 3,
        "Tab 2 三 pane id 必须唯一"
    );
    assert_authoritative_focus(workspace, tab2, active_pane(workspace)?, "向下 split 后")?;

    let all_panes = [tab1_pane, tab2_panes[0], tab2_panes[1], tab2_panes[2]];
    switch_tab(workspace, tab1, "切 Tab 1")?;
    switch_pane(workspace, tab1_pane, "切 Tab 1 pane")?;
    assert_authoritative_focus(workspace, tab1, tab1_pane, "切到 Tab 1")?;
    execute_and_assert_three_way_binding(
        workspace,
        tab1_pane,
        &all_panes,
        &format!("{}_T1", transport.to_ascii_uppercase()),
    )?;

    switch_tab(workspace, tab2, "切 Tab 2")?;
    for (index, pane) in tab2_panes.iter().copied().enumerate() {
        switch_pane(workspace, pane, &format!("切 Tab 2 pane {}", index + 1))?;
        assert_authoritative_focus(workspace, tab2, pane, &format!("Tab 2 pane {}", index + 1))?;
        execute_and_assert_three_way_binding(
            workspace,
            pane,
            &all_panes,
            &format!("{}_T2_P{}", transport.to_ascii_uppercase(), index + 1),
        )?;
    }

    rt.block_on(workspace.shutdown())?;
    Ok(())
}

#[test]
fn local_and_ssh_herdr_focus_and_observer_binding_match_server_authority() {
    assert!(herdr_available(), "Herdr authority contract 要求 herdr");
    assert!(
        loopback_sshd_available(),
        "Herdr authority contract 要求可自启 loopback sshd"
    );
    let sshd = LoopbackSshd::start("herdr-authority").expect("启动 loopback sshd");
    let _ssh_config = EnvRestore::set("MUXTERM_SSH_CONFIG_PATH", &sshd.config_path);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("Tokio runtime");

    for transport in ["local", "ssh"] {
        run_case(&rt, &sshd, transport)
            .unwrap_or_else(|error| panic!("Herdr {transport} authority contract: {error:#}"));
    }
}
