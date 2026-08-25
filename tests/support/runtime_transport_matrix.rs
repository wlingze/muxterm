#![allow(dead_code)]
//! Runtime x Transport 的共享行为契约。
//!
//! 这里不识别 tmux / Herdr wire id；只通过产品层 `Workspace`、`Task`、
//! `State` 验证所有 Runtime 都必须提供的 Tab/Pane/输入/焦点语义。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};

use muxterm::core::model::backend::RuntimeCapability;
use muxterm::core::model::layout::{LayoutNode, SplitDir};
use muxterm::core::model::state::{BackendStatus, MutationResult, StateChange};
use muxterm::core::model::task::{Task, TaskOutcome};
use muxterm::core::protocol::terminal::emulate::TerminalState;
use muxterm::core::types::{PaneId, TabId};
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::core::workspace::workspace::Workspace;

use super::herdr_test_support::IsolatedHerdr;
use super::sshd_test_support::LoopbackSshd;
use super::tmux_test_support::{create_session, kill_server, unique_socket};

pub const MATRIX_TIMEOUT: Duration = Duration::from_secs(15);

enum MatrixGuard {
    Shell,
    Tmux { socket: String },
    Herdr { _guard: IsolatedHerdr },
}

impl Drop for MatrixGuard {
    fn drop(&mut self) {
        if let Self::Tmux { socket } = self {
            kill_server(socket);
        }
    }
}

/// 一格 Runtime × Transport 的两个真实 Workspace fixture。
pub struct MatrixFixture {
    pub spec: WorkspaceSpec,
    pub alternate_spec: WorkspaceSpec,
    _guard: MatrixGuard,
}

impl MatrixFixture {
    pub fn new(runtime: &str, transport: &str, sshd: &LoopbackSshd) -> Result<Self> {
        match (runtime, transport) {
            ("shell", "local") => Ok(Self {
                spec: WorkspaceSpec::local_shell("/tmp"),
                alternate_spec: WorkspaceSpec::local_shell("/"),
                _guard: MatrixGuard::Shell,
            }),
            ("shell", "ssh") => Ok(Self {
                spec: WorkspaceSpec::ssh_shell(sshd.alias.clone(), "/tmp"),
                alternate_spec: WorkspaceSpec::ssh_shell(sshd.alias.clone(), "/"),
                _guard: MatrixGuard::Shell,
            }),
            ("tmux", "local") | ("tmux", "ssh") => {
                // W7：parent 预分配 socket 名时 child 必须复用，兜底清理才能命中。
                let socket = std::env::var("MUXTERM_TEST_FIXTURE_SOCKET")
                    .unwrap_or_else(|_| unique_socket(&format!("matrix-{runtime}-{transport}")));
                let session = format!("matrix-{runtime}-{transport}");
                let alternate_session = format!("matrix-{runtime}-{transport}-alternate");
                create_session(&socket, &session, 120, 40);
                create_session(&socket, &alternate_session, 120, 40);
                let spec = if transport == "ssh" {
                    WorkspaceSpec::ssh_tmux(sshd.alias.clone(), Some(session), Some(socket.clone()))
                } else {
                    WorkspaceSpec::local_tmux(Some(session), Some(socket.clone()))
                };
                let alternate_spec = if transport == "ssh" {
                    WorkspaceSpec::ssh_tmux(
                        sshd.alias.clone(),
                        Some(alternate_session),
                        Some(socket.clone()),
                    )
                } else {
                    WorkspaceSpec::local_tmux(Some(alternate_session), Some(socket.clone()))
                };
                Ok(Self {
                    spec,
                    alternate_spec,
                    _guard: MatrixGuard::Tmux { socket },
                })
            }
            ("herdr", "local") | ("herdr", "ssh") => {
                // W7：parent 预分配精确 session 名时 child 复用（兜底清理可命中）。
                let herdr = match std::env::var("MUXTERM_TEST_FIXTURE_NAME") {
                    Ok(name) => IsolatedHerdr::start_named(name),
                    Err(_) => IsolatedHerdr::start(&format!("matrix-{transport}")),
                };
                let (workspace, _, _) =
                    herdr.create_workspace("/tmp", &format!("matrix-{transport}"));
                let (alternate_workspace, _, _) =
                    herdr.create_workspace("/", &format!("matrix-{transport}-alternate"));
                let spec = if transport == "ssh" {
                    WorkspaceSpec::ssh_herdr(
                        sshd.alias.clone(),
                        herdr.name(),
                        workspace,
                        herdr.socket_path().to_string_lossy(),
                    )
                } else {
                    WorkspaceSpec::herdr(
                        herdr.name(),
                        workspace,
                        herdr.socket_path().to_string_lossy(),
                    )
                };
                let alternate_spec = if transport == "ssh" {
                    WorkspaceSpec::ssh_herdr(
                        sshd.alias.clone(),
                        herdr.name(),
                        alternate_workspace,
                        herdr.socket_path().to_string_lossy(),
                    )
                } else {
                    WorkspaceSpec::herdr(
                        herdr.name(),
                        alternate_workspace,
                        herdr.socket_path().to_string_lossy(),
                    )
                };
                Ok(Self {
                    spec,
                    alternate_spec,
                    _guard: MatrixGuard::Herdr { _guard: herdr },
                })
            }
            _ => anyhow::bail!(
                "新增组合 {runtime} x {transport} 尚无真实 fixture；必须补齐后才能更新注册表"
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScenarioSnapshot {
    pub tab1_token: String,
    pub tab2_tokens: Vec<String>,
    pub persistent: bool,
}

fn rendered_pane_output(workspace: &Workspace, pane: PaneId) -> String {
    let Some(info) = workspace.state().pane(&pane) else {
        return String::new();
    };
    let bytes = workspace.state().pane_output(&pane).unwrap_or_default();
    let mut terminal = TerminalState::new(info.cols.max(1) as usize, info.rows.max(1) as usize);
    terminal.feed(bytes);
    terminal.last_n_lines(10_000).join("\n")
}

fn pane_contains_token(workspace: &Workspace, pane: PaneId, token: &str) -> bool {
    let raw_contains = workspace
        .state()
        .pane_output(&pane)
        .is_some_and(|bytes| String::from_utf8_lossy(bytes).contains(token));
    let rendered = rendered_pane_output(workspace, pane);
    let compact = rendered
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    raw_contains || rendered.contains(token) || compact.contains(token)
}

fn assert_tokens_belong_to_one_pane(
    workspace: &Workspace,
    tokens: impl IntoIterator<Item = String>,
) -> Result<()> {
    let mut locations = Vec::new();
    let mut output_diagnostics = Vec::new();
    let tokens = tokens.into_iter().collect::<Vec<_>>();
    for tab in workspace.state().tabs() {
        for pane in workspace.state().panes(&tab.id) {
            let bytes = workspace.state().pane_output(&pane.id).unwrap_or_default();
            let output = rendered_pane_output(workspace, pane.id);
            let preview = output
                .chars()
                .rev()
                .take(600)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>();
            output_diagnostics.push((pane.id, bytes.len(), preview.escape_debug().to_string()));
            for token in &tokens {
                if pane_contains_token(workspace, pane.id, token) {
                    locations.push((token.clone(), pane.id));
                }
            }
        }
    }
    for token in tokens {
        ensure!(
            locations
                .iter()
                .filter(|(found, _)| found == &token)
                .count()
                == 1,
            "token {token} 必须只属于一个 pane，locations={locations:?}, outputs={output_diagnostics:?}"
        );
    }
    Ok(())
}

fn wait_until(
    workspace: &mut Workspace,
    label: &str,
    mut predicate: impl FnMut(&Workspace) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + MATRIX_TIMEOUT;
    while Instant::now() < deadline {
        let _ = workspace.refresh();
        if predicate(workspace) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!(
        "{label} 超时: runtime={} tabs={:?} active_tab={:?} active_pane={:?}",
        workspace.runtime().workspace_runtime(),
        workspace
            .state()
            .tabs()
            .iter()
            .map(|tab| (tab.id, tab.active))
            .collect::<Vec<_>>(),
        workspace.state().active_tab().map(|tab| tab.id),
        workspace.state().active_pane().map(|pane| pane.id),
    )
}

/// 单条事件是否就是目标 operation 的 Completed settlement。
fn settle_check(event: StateChange, operation_id: u64, label: &str) -> Result<Option<()>> {
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

fn done(workspace: &mut Workspace, task: Task, label: &str) -> Result<()> {
    let outcome = workspace
        .execute(task)
        .with_context(|| format!("{label} 执行失败"))?;
    match outcome {
        // shell/tmux 同步完成。
        TaskOutcome::Done => Ok(()),
        // W5：herdr 异步 mutation —— 必须等到唯一 MutationSettled(Completed)。
        TaskOutcome::Accepted { operation_id } => {
            let deadline = Instant::now() + MATRIX_TIMEOUT;
            while Instant::now() < deadline {
                // refresh() 会返回新事件（take_events 只排空已 poll 的）；两路都查。
                for event in workspace.take_events() {
                    if let Some(()) = settle_check(event, operation_id, label)? {
                        return Ok(());
                    }
                }
                for event in workspace.refresh() {
                    if let Some(()) = settle_check(event, operation_id, label)? {
                        return Ok(());
                    }
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            anyhow::bail!("{label} 等待 MutationSettled 超时")
        }
        other => anyhow::bail!("{label} 必须成功，实际 {other:?}"),
    }
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

fn assert_right_nested_layout(workspace: &Workspace, tab: TabId) -> Result<[PaneId; 3]> {
    let layout = workspace
        .state()
        .layout(&tab)
        .with_context(|| format!("tab {tab} 缺 layout"))?;
    let (left, right_top, right_bottom) = match &layout.tree {
        LayoutNode::Split {
            dir: SplitDir::Horizontal,
            first,
            second,
            ..
        } => {
            let LayoutNode::Leaf(left) = first.as_ref() else {
                anyhow::bail!("三 pane 左侧必须是单 leaf，实际 {first:?}");
            };
            let LayoutNode::Split {
                dir: SplitDir::Vertical,
                first: right_top,
                second: right_bottom,
                ..
            } = second.as_ref()
            else {
                anyhow::bail!("三 pane 右侧必须上下分割，实际 {second:?}");
            };
            let LayoutNode::Leaf(right_top) = right_top.as_ref() else {
                anyhow::bail!("右上必须是 leaf，实际 {right_top:?}");
            };
            let LayoutNode::Leaf(right_bottom) = right_bottom.as_ref() else {
                anyhow::bail!("右下必须是 leaf，实际 {right_bottom:?}");
            };
            (*left, *right_top, *right_bottom)
        }
        tree => anyhow::bail!("三 pane 必须是 H(left,V(right-top,right-bottom))，实际 {tree:?}"),
    };
    ensure!(
        HashSet::from([left, right_top, right_bottom]).len() == 3,
        "三 pane id 必须互不相同"
    );
    Ok([left, right_top, right_bottom])
}

fn switch_tab_stable(workspace: &mut Workspace, target: TabId, label: &str) -> Result<()> {
    done(workspace, Task::SwitchTab { target }, label)?;
    wait_until(workspace, label, |ws| {
        ws.state().active_tab().map(|tab| tab.id) == Some(target)
    })?;
    // 再消费一段后端事件，防止本地乐观焦点随后被 authoritative snapshot 改回。
    let stable_until = Instant::now() + Duration::from_millis(350);
    while Instant::now() < stable_until {
        let _ = workspace.refresh();
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        workspace.state().active_tab().map(|tab| tab.id) == Some(target),
        "{label}: 后端事件把 active tab 改回了 {:?}",
        workspace.state().active_tab().map(|tab| tab.id)
    );
    Ok(())
}

fn switch_pane_stable(workspace: &mut Workspace, target: PaneId, label: &str) -> Result<()> {
    done(workspace, Task::SwitchPane { target }, label)?;
    wait_until(workspace, label, |ws| {
        ws.state().active_pane().map(|pane| pane.id) == Some(target)
    })?;
    let stable_until = Instant::now() + Duration::from_millis(250);
    while Instant::now() < stable_until {
        let _ = workspace.refresh();
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        workspace.state().active_pane().map(|pane| pane.id) == Some(target),
        "{label}: 后端事件把 active pane 改回了 {:?}",
        workspace.state().active_pane().map(|pane| pane.id)
    );
    Ok(())
}

fn execute_echo(
    workspace: &mut Workspace,
    target: PaneId,
    token_suffix: &str,
    all_panes: &[PaneId],
) -> Result<String> {
    let token = format!("MX_{token_suffix}");
    // 输入中没有连续 token；只有 shell 真正执行 printf 后，输出才包含它。
    let command = format!("printf 'MX_%s\\n' '{token_suffix}'\r");
    ensure!(
        !command.contains(&token),
        "测试命令不得把期望 token 原样写进输入回显"
    );
    // 远端 shell/Herdr control stream 在 Workspace connect 返回后仍可能处于
    // startup；一次性写入会在慢 CI 丢失，而本地快 runner 偶然成功。重试是
    // 有界的 fixture readiness 处理，断言仍要求真实命令输出 token。
    let deadline = Instant::now() + MATRIX_TIMEOUT;
    let mut next_send = Instant::now();
    let mut delivered = false;
    while Instant::now() < deadline {
        if Instant::now() >= next_send {
            done(
                workspace,
                Task::WriteRaw {
                    target,
                    data: command.as_bytes().to_vec(),
                },
                &format!("pane {target} printf"),
            )?;
            next_send = Instant::now() + Duration::from_millis(500);
        }
        let _ = workspace.refresh();
        if pane_contains_token(workspace, target, &token) {
            delivered = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if !delivered {
        let bytes = workspace.state().pane_output(&target).unwrap_or_default();
        let raw_tail = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(1_200)..]);
        let rendered = rendered_pane_output(workspace, target);
        anyhow::bail!(
            "pane {target} 输出 {token} 超时; raw_len={}, raw_tail={}, rendered={}",
            bytes.len(),
            raw_tail.escape_debug(),
            rendered.escape_debug()
        );
    }
    for pane in all_panes {
        let contains = pane_contains_token(workspace, *pane, &token);
        ensure!(
            (*pane == target) == contains,
            "token {token} 输出归属错误: target={target}, pane={pane}, contains={contains}"
        );
    }
    Ok(token)
}

/// 首次连接后构造标准场景，并验证每次 tab/pane 切换与真实命令输出。
pub fn build_2tab3pane(
    workspace: &mut Workspace,
    runtime: &str,
    transport: &str,
) -> Result<ScenarioSnapshot> {
    wait_until(workspace, "初始单 tab/single pane", |ws| {
        ws.state().tabs().len() == 1
            && ws
                .state()
                .active_tab()
                .is_some_and(|tab| leaves(ws, tab.id).len() == 1)
    })?;
    let tab1 = active_tab(workspace)?;
    let tab1_pane = active_pane(workspace)?;

    done(
        workspace,
        Task::NewTab {
            name: Some("matrix-tab-2".into()),
            command: None,
            workdir: None,
        },
        "创建 Tab 2",
    )?;
    wait_until(workspace, "创建 Tab 2", |ws| {
        ws.state().tabs().len() == 2
            && ws.state().active_tab().is_some_and(|tab| tab.id != tab1)
            && ws.state().active_pane().is_some_and(|pane| {
                ws.state()
                    .active_tab()
                    .is_some_and(|tab| pane.tab == tab.id)
            })
    })?;
    let tab2 = active_tab(workspace)?;
    let left = active_pane(workspace)?;

    done(
        workspace,
        Task::SplitPane {
            target: Some(left),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        },
        "Tab 2 向右分割",
    )?;
    wait_until(workspace, "Tab 2 两 pane", |ws| {
        leaves(ws, tab2).len() == 2
    })?;
    let right_top = leaves(workspace, tab2)
        .into_iter()
        .find(|pane| *pane != left)
        .context("向右分割后缺新 pane")?;
    ensure!(
        active_pane(workspace)? == right_top,
        "向右分割后必须聚焦新建的右 pane"
    );

    done(
        workspace,
        Task::SplitPane {
            target: Some(right_top),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        },
        "Tab 2 右 pane 向下分割",
    )?;
    wait_until(workspace, "Tab 2 三 pane", |ws| {
        leaves(ws, tab2).len() == 3
    })?;
    let tab2_panes = assert_right_nested_layout(workspace, tab2)?;

    let pair = format!(
        "{}_{}",
        runtime.replace('-', "_"),
        transport.replace('-', "_")
    )
    .to_ascii_uppercase();
    let all_panes = [tab1_pane, tab2_panes[0], tab2_panes[1], tab2_panes[2]];

    switch_tab_stable(workspace, tab1, "切换 Tab 1")?;
    switch_pane_stable(workspace, tab1_pane, "切换 Tab 1 pane")?;
    let tab1_token = execute_echo(workspace, tab1_pane, &format!("{pair}_T1_P1"), &all_panes)?;

    switch_tab_stable(workspace, tab2, "切换 Tab 2")?;
    let mut tab2_tokens = Vec::new();
    for (index, pane) in tab2_panes.into_iter().enumerate() {
        switch_pane_stable(workspace, pane, &format!("切换 Tab 2 pane {}", index + 1))?;
        tab2_tokens.push(execute_echo(
            workspace,
            pane,
            &format!("{pair}_T2_P{}", index + 1),
            &all_panes,
        )?);
    }

    // detach 前最终焦点固定在三 pane 的 Tab 2，重连必须从后端恢复这里。
    switch_tab_stable(workspace, tab2, "detach 前切回 Tab 2")?;
    switch_pane_stable(workspace, tab2_panes[2], "detach 前切到右下 pane")?;

    Ok(ScenarioSnapshot {
        tab1_token,
        tab2_tokens,
        persistent: workspace
            .runtime()
            .support()
            .contains(&RuntimeCapability::PersistDetach),
    })
}

/// 第二个 Workspace 必须真正可用；在其中执行一次命令，供切回后检查跨 Workspace 隔离。
pub fn verify_fresh_workspace(
    workspace: &mut Workspace,
    runtime: &str,
    transport: &str,
) -> Result<String> {
    wait_until(
        workspace,
        "第二个 Workspace 初始单 tab/single pane",
        |ws| {
            ws.state().tabs().len() == 1
                && ws
                    .state()
                    .active_tab()
                    .is_some_and(|tab| leaves(ws, tab.id).len() == 1)
        },
    )?;
    let pane = active_pane(workspace)?;
    let pair = format!(
        "{}_{}_POOL_ALT",
        runtime.replace('-', "_"),
        transport.replace('-', "_")
    )
    .to_ascii_uppercase();
    execute_echo(workspace, pane, &pair, &[pane])
}

/// SSH shell 必须真正在远端 PTY 内运行；本地 fallback 没有 `SSH_CONNECTION`。
pub fn verify_ssh_shell_transport(workspace: &mut Workspace) -> Result<()> {
    let pane = active_pane(workspace)?;
    let suffix = "SHELL_SSH_REMOTE_ENV";
    let token = format!("MX_{suffix}");
    let command = format!("if [ -n \"$SSH_CONNECTION\" ]; then printf 'MX_%s\\n' '{suffix}'; fi\r");
    ensure!(
        !command.contains(&token),
        "SSH transport 证明命令不得把期望 token 原样写进输入回显"
    );
    done(
        workspace,
        Task::WriteRaw {
            target: pane,
            data: command.into_bytes(),
        },
        "SSH shell 远端环境证明",
    )?;
    wait_until(workspace, "SSH shell 必须存在 SSH_CONNECTION", |ws| {
        pane_contains_token(ws, pane, &token)
    })
}

/// 从第二个 Workspace 通过 WorkspacePool 切回后，原对象的拓扑、焦点和输出必须保留。
pub fn verify_after_pool_switch(
    workspace: &mut Workspace,
    runtime: &str,
    transport: &str,
    before: &ScenarioSnapshot,
    alternate_token: &str,
) -> Result<()> {
    wait_until(workspace, "WorkspacePool 切回后恢复 2 tab", |ws| {
        ws.state().tabs().len() == 2
    })?;
    let active = active_tab(workspace)?;
    wait_until(
        workspace,
        "WorkspacePool 切回后 active Tab 2 三 pane",
        |ws| leaves(ws, active).len() == 3,
    )?;
    let panes = assert_right_nested_layout(workspace, active)?;
    ensure!(
        active_pane(workspace)? == panes[2],
        "WorkspacePool 切回后必须保持右下 active pane，实际 {:?}",
        active_pane(workspace)?
    );
    let other = workspace
        .state()
        .tabs()
        .into_iter()
        .find(|tab| tab.id != active)
        .map(|tab| tab.id)
        .context("WorkspacePool 切回后缺 Tab 1")?;
    ensure!(
        leaves(workspace, other).len() == 1,
        "WorkspacePool 切回后 Tab 1 必须保持单 pane"
    );
    assert_tokens_belong_to_one_pane(
        workspace,
        std::iter::once(before.tab1_token.clone()).chain(before.tab2_tokens.iter().cloned()),
    )?;
    for tab in workspace.state().tabs() {
        for pane in workspace.state().panes(&tab.id) {
            let contains_alternate = pane_contains_token(workspace, pane.id, alternate_token);
            ensure!(
                !contains_alternate,
                "第二个 Workspace 的 token {alternate_token} 串入原 Workspace pane {}",
                pane.id
            );
        }
    }

    let all_panes = [leaves(workspace, other)[0], panes[0], panes[1], panes[2]];
    let pair = format!(
        "{}_{}_POOL_RETURN",
        runtime.replace('-', "_"),
        transport.replace('-', "_")
    )
    .to_ascii_uppercase();
    let _ = execute_echo(workspace, panes[2], &pair, &all_panes)?;
    ensure!(
        workspace.runtime().runtime_status() == BackendStatus::Connected,
        "WorkspacePool 切回后 Runtime 必须 Connected"
    );
    Ok(())
}

/// attach 后验证 authoritative 拓扑/焦点和既有输出，并再次逐 pane 执行命令。
pub fn verify_after_attach(
    workspace: &mut Workspace,
    runtime: &str,
    transport: &str,
    before: &ScenarioSnapshot,
) -> Result<()> {
    wait_until(workspace, "attach 后恢复 2 tab", |ws| {
        ws.state().tabs().len() == 2
    })?;
    let active = active_tab(workspace)?;
    wait_until(workspace, "attach 后 active Tab 2 三 pane", |ws| {
        leaves(ws, active).len() == 3
    })?;
    let panes = assert_right_nested_layout(workspace, active)?;
    ensure!(
        active_pane(workspace)? == panes[2],
        "attach 后必须恢复 detach 前的右下 active pane，实际 {:?}",
        active_pane(workspace)?
    );

    let other = workspace
        .state()
        .tabs()
        .into_iter()
        .find(|tab| tab.id != active)
        .map(|tab| tab.id)
        .context("attach 后缺 Tab 1")?;
    ensure!(
        leaves(workspace, other).len() == 1,
        "attach 后 Tab 1 必须保持单 pane"
    );

    if before.persistent {
        assert_tokens_belong_to_one_pane(
            workspace,
            std::iter::once(before.tab1_token.clone()).chain(before.tab2_tokens.iter().cloned()),
        )?;
    }

    // Attach 后继续 mutation 是独立生命周期边界：先在已恢复的 active
    // pane 上 split，再对新 pane split，最后创建新 tab。这个顺序覆盖
    // Herdr attach → SplitPane → 新 pane bootstrap 的崩溃路径，也要求
    // tmux/Herdr 在同一套 Task contract 下收敛。
    let attached_tab = active;
    let attached_pane = active_pane(workspace)?;
    done(
        workspace,
        Task::SplitPane {
            target: Some(attached_pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        },
        "attach 后第一次水平 split",
    )?;
    wait_until(workspace, "attach 后水平 split", |ws| {
        leaves(ws, attached_tab).len() == 4
    })?;
    let first_created = active_pane(workspace)?;
    ensure!(
        first_created != attached_pane,
        "attach 后水平 split 必须聚焦新 pane"
    );

    done(
        workspace,
        Task::SplitPane {
            target: Some(first_created),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        },
        "attach 后第二次垂直 split",
    )?;
    wait_until(workspace, "attach 后垂直 split", |ws| {
        leaves(ws, attached_tab).len() == 5
    })?;
    let attached_panes = leaves(workspace, attached_tab);
    ensure!(
        active_pane(workspace)? != first_created,
        "attach 后第二次 split 必须聚焦第二个新 pane"
    );

    done(
        workspace,
        Task::NewTab {
            name: Some("attach-after-mutation".into()),
            command: None,
            workdir: None,
        },
        "attach 后创建新 tab",
    )?;
    wait_until(workspace, "attach 后新 tab", |ws| {
        ws.state().tabs().len() == 3
            && ws
                .state()
                .active_tab()
                .is_some_and(|tab| tab.id != attached_tab)
            && ws.state().active_pane().is_some()
    })?;
    let created_tab = active_tab(workspace)?;
    let created_tab_pane = active_pane(workspace)?;
    let all_panes = attached_panes
        .iter()
        .copied()
        .chain(std::iter::once(created_tab_pane))
        .collect::<Vec<_>>();
    ensure!(
        all_panes.len() == 6,
        "attach 后最终 topology 必须是 3 tabs/6 panes"
    );

    for (index, pane) in all_panes.iter().copied().enumerate() {
        let tab = workspace
            .state()
            .pane(&pane)
            .map(|info| info.tab)
            .context("attach mutation 后新 pane 缺 tab")?;
        switch_tab_stable(workspace, tab, &format!("attach mutation 切 tab {tab}"))?;
        switch_pane_stable(workspace, pane, &format!("attach mutation 切 pane {pane}"))?;
        let _ = execute_echo(workspace, pane, &format!("ATT_P{}", index + 1), &all_panes)?;
    }
    switch_tab_stable(workspace, created_tab, "attach mutation 回新 tab")?;
    switch_pane_stable(workspace, created_tab_pane, "attach mutation 回新 tab pane")?;

    let pair = format!(
        "{}_{}_REATTACH",
        runtime.replace('-', "_"),
        transport.replace('-', "_")
    )
    .to_ascii_uppercase();
    let tab1_pane = leaves(workspace, other)[0];
    let all_panes = [tab1_pane, panes[0], panes[1], panes[2]];
    switch_tab_stable(workspace, other, "attach 后切 Tab 1")?;
    switch_pane_stable(workspace, tab1_pane, "attach 后切 Tab 1 pane")?;
    let _ = execute_echo(workspace, tab1_pane, &format!("{pair}_T1"), &all_panes)?;
    switch_tab_stable(workspace, active, "attach 后切 Tab 2")?;
    for (index, pane) in panes.into_iter().enumerate() {
        switch_pane_stable(
            workspace,
            pane,
            &format!("attach 后切 Tab 2 pane {}", index + 1),
        )?;
        let _ = execute_echo(
            workspace,
            pane,
            &format!("{pair}_T2_P{}", index + 1),
            &all_panes,
        )?;
    }
    ensure!(
        workspace.runtime().runtime_status() == BackendStatus::Connected,
        "attach 后 Runtime 必须 Connected"
    );
    Ok(())
}
