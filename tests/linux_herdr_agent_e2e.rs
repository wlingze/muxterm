//! Herdr agent lifecycle 与终端连续性的 GTK 端到端契约。
//!
//! local/SSH 都先经生产 Action 建 2 tab/3 pane、逐 pane 执行命令，再让真实
//! pi 进程经历 working/blocked/done。每个命令同时核对 Herdr pane.read、
//! Workspace PaneBuf/search 与唯一目标 VTE，防止 agent 消息测试掩盖终端回归。

#![cfg(feature = "gtk")]

mod support;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use gtk4::prelude::*;

use muxterm::core::config::{Action, Config};
use muxterm::core::model::backend::RuntimeCapability;
use muxterm::core::model::task::TaskOutcome;
use muxterm::core::runtime::herdr::session::{HerdrAgentStatus, HerdrSession};
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;
use support::herdr_test_support::{herdr_available, IsolatedHerdr, TempAgentCommand};
use support::linux_gtk::{gtk_test_framework_smoke, load_theme, pump_main_loop, skip_no_display};
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};

const HERDR_TIMEOUT: Duration = Duration::from_secs(25);

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

#[derive(Debug)]
struct Scenario {
    tab1: u32,
    tab1_pane: u32,
    wire_tab1: String,
    tab2: u32,
    tab2_panes: [u32; 3],
    wire_tab2: String,
    pane_map: HashMap<u32, String>,
}

#[derive(Clone, Copy)]
struct Authority<'a> {
    session: &'a HerdrSession,
    workspace_id: &'a str,
}

#[derive(Clone, Copy)]
struct FocusTarget<'a> {
    product_tab: u32,
    product_pane: u32,
    wire_tab: &'a str,
    wire_pane: &'a str,
}

#[derive(Debug, Clone)]
struct BoundToken {
    product_pane: u32,
    wire_pane: String,
    token: String,
}

fn tick(app: &AppWindow) {
    app.test_poll_once();
    pump_main_loop(25);
    app.test_flush_feeds();
}

fn diagnostics(app: &AppWindow) -> String {
    format!(
        "workspace={} runtime={} tab={} pane={} leaves={:?} gtk={} ids={:?}",
        app.test_active_workspace_replica_id(),
        app.test_active_workspace_runtime(),
        app.test_active_tab_id(),
        app.test_active_pane_id(),
        app.test_layout_leaf_ids(),
        app.test_gtk_layout_signature(),
        app.test_workspace_replica_ids(),
    )
}

/// 轮询等待服务端 pane.read（recent 2000 行）出现指定文本。
/// reopen/reattach 后服务端 replay 异步完成，CI 慢环境下 token 行可能
/// 延迟回到 buffer 尾部；单次读取会时序误报（detach/reattach 连续性）。
fn wait_for_server_text(session: &HerdrSession, wire_pane: &str, text: &str) -> bool {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(bytes) = session.pane_read_recent_ansi_lines(wire_pane, 2000) {
            if String::from_utf8_lossy(&bytes).contains(text) {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn wait_for(
    app: &AppWindow,
    label: &str,
    mut predicate: impl FnMut(&AppWindow) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        tick(app);
        if predicate(app) {
            return Ok(());
        }
    }
    anyhow::bail!("等待 {label} 超时: {}", diagnostics(app))
}

fn server_focus(session: &HerdrSession, workspace_id: &str) -> Result<(String, String)> {
    let snapshot = session.snapshot()?;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|candidate| candidate.workspace_id == workspace_id)
        .context("服务端 snapshot 缺目标 workspace")?;
    let tab = workspace
        .active_tab_id
        .clone()
        .context("服务端 workspace 缺 active_tab_id")?;
    let pane = snapshot
        .layouts
        .iter()
        .find(|layout| layout.tab_id == tab)
        .map(|layout| layout.focused_pane_id.clone())
        .context("服务端 active tab 缺 focused pane")?;
    ensure!(
        snapshot.focused_workspace_id.as_deref() == Some(workspace_id),
        "服务端 focused workspace={:?}, expected={workspace_id}",
        snapshot.focused_workspace_id
    );
    ensure!(
        snapshot.focused_tab_id.as_deref() == Some(tab.as_str())
            && snapshot.focused_pane_id.as_deref() == Some(pane.as_str()),
        "服务端全局焦点与 workspace/layout 不一致: tab={tab}, pane={pane}, snapshot={snapshot:#?}"
    );
    Ok((tab, pane))
}

fn assert_focus(
    app: &AppWindow,
    authority: Authority<'_>,
    target: FocusTarget<'_>,
    label: &str,
) -> Result<()> {
    ensure!(
        app.test_active_tab_id() == target.product_tab
            && app.test_active_pane_id() == target.product_pane
            && app.test_layout_leaf_ids().contains(&target.product_pane),
        "{label}: Workspace/GTK 焦点错误，expected tab={} pane={}; {}",
        target.product_tab,
        target.product_pane,
        diagnostics(app)
    );
    let (actual_tab, actual_pane) = server_focus(authority.session, authority.workspace_id)?;
    ensure!(
        actual_tab == target.wire_tab && actual_pane == target.wire_pane,
        "{label}: Herdr 焦点错误，expected tab={} pane={}, actual tab={actual_tab} pane={actual_pane}",
        target.wire_tab,
        target.wire_pane,
    );
    Ok(())
}

fn emit_command(app: &AppWindow, command: &str) -> Result<()> {
    for character in command.chars() {
        ensure!(
            app.test_emit_active_pane_commit(&character.to_string()),
            "active VTE commit 路径不存在: {}",
            diagnostics(app)
        );
    }
    ensure!(
        app.test_emit_active_pane_commit("\r"),
        "active VTE Enter 路径不存在"
    );
    Ok(())
}

fn server_log_tail(session: &HerdrSession) -> String {
    let log = session
        .socket_path()
        .parent()
        .map(|dir| dir.join("herdr-server.log"));
    let Some(log) = log.filter(|p| p.exists()) else {
        return "（无 herdr-server.log）".into();
    };
    match std::fs::read_to_string(&log) {
        // 失败诊断打印完整日志：CI 无人工轮询，40 行 tail 往往截掉根因。
        Ok(text) => text,
        Err(err) => format!("（读 herdr-server.log 失败: {err}）"),
    }
}

fn assert_bound_token(
    app: &AppWindow,
    session: &HerdrSession,
    pane_map: &HashMap<u32, String>,
    bound: &BoundToken,
) -> Result<()> {
    // reopen/reattach 后服务端对 pane 的 replay 是异步的：新客户端 attach
    // 触发服务端重放/重绘，CI（慢环境）里 token 行可能在 attach 完成瞬间
    // 尚未回到 buffer 尾部。轮询等待而不是单次 ensure，避免时序误报。
    let server_ok = wait_for_server_text(session, &bound.wire_pane, &bound.token);
    ensure!(
        server_ok,
        "服务端 pane.read({}) 缺 {}；服务端内容: {:?}\nherdr-server.log tail:\n{}",
        bound.wire_pane,
        bound.token,
        String::from_utf8_lossy(&session.pane_read_recent_ansi_lines(&bound.wire_pane, 2000)?)
            .escape_debug(),
        server_log_tail(session),
    );
    let hit_panes = app
        .test_search_workspace(&bound.token)
        .into_iter()
        .map(|(_, pane, _)| pane)
        .collect::<HashSet<_>>();
    ensure!(
        hit_panes == HashSet::from([bound.product_pane]),
        "Workspace token {} 归属错误: {hit_panes:?}",
        bound.token
    );
    ensure!(
        app.test_pane_vte_text(bound.product_pane)
            .contains(&bound.token),
        "目标 VTE {} 缺 {}",
        bound.product_pane,
        bound.token
    );
    for (product, wire) in pane_map {
        if *product == bound.product_pane {
            continue;
        }
        ensure!(
            !String::from_utf8_lossy(&session.pane_read_ansi(wire)?).contains(&bound.token),
            "服务端 token {} 串到 {wire}",
            bound.token
        );
        ensure!(
            !app.test_pane_vte_text(*product).contains(&bound.token),
            "VTE token {} 串到产品 pane {product}",
            bound.token
        );
    }
    Ok(())
}

fn execute_and_assert_binding(
    app: &AppWindow,
    session: &HerdrSession,
    pane_map: &HashMap<u32, String>,
    suffix: &str,
) -> Result<BoundToken> {
    let product_pane = app.test_active_pane_id();
    let wire_pane = pane_map
        .get(&product_pane)
        .with_context(|| format!("产品 pane {product_pane} 缺 Herdr mapping"))?
        .clone();
    // 三分屏最窄 pane 可能只有二十余列；token 必须能在一行完整出现，
    // 否则 reattach 后 pane.read 的正确软换行会把连续子串拆开。
    let token = format!("HA_{suffix}");
    let command = format!("printf 'HA_%s\\n' '{suffix}'");
    ensure!(!command.contains(&token), "输入不得原样包含期望 token");
    ensure!(
        app.test_search_workspace(&token).is_empty(),
        "执行前 Workspace 不得已有 {token}"
    );
    app.test_clear_active_pane_render_trace();
    emit_command(app, &command)?;

    let bound = BoundToken {
        product_pane,
        wire_pane,
        token,
    };
    let delivery = wait_for(app, &format!("三方收到 {}", bound.token), |candidate| {
        session
            .pane_read_recent_ansi_lines(&bound.wire_pane, 2000)
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(&bound.token))
            && candidate
                .test_search_workspace(&bound.token)
                .iter()
                .any(|(_, pane, line)| *pane == bound.product_pane && line.contains(&bound.token))
            && candidate
                .test_pane_vte_text(bound.product_pane)
                .contains(&bound.token)
    });
    if let Err(error) = delivery {
        let server = session
            .pane_read_recent_ansi_lines(&bound.wire_pane, 2000)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_else(|read_error| format!("pane.read error: {read_error:#}"));
        let vte = app.test_pane_vte_text(bound.product_pane);
        anyhow::bail!(
            "{error:#}; token={}; server={}; search={:?}; vte={}",
            bound.token,
            server.escape_debug(),
            app.test_search_workspace(&bound.token),
            vte.escape_debug(),
        );
    }
    assert_bound_token(app, session, pane_map, &bound)?;
    ensure!(
        app.test_active_pane_id() == product_pane && app.test_active_pane_resets() == 0,
        "命令执行不得切 pane/reset Surface: pane={product_pane}, resets={}, {}",
        app.test_active_pane_resets(),
        diagnostics(app)
    );
    Ok(bound)
}

fn switch_tab(
    app: &AppWindow,
    action: Action,
    authority: Authority<'_>,
    target: FocusTarget<'_>,
    label: &str,
) -> Result<()> {
    app.test_handle_action(action);
    wait_for(app, label, |candidate| {
        candidate.test_active_tab_id() == target.product_tab
            && candidate.test_active_pane_id() == target.product_pane
            && candidate
                .test_layout_leaf_ids()
                .contains(&target.product_pane)
    })?;
    assert_focus(app, authority, target, label)
}

fn focus_pane(
    app: &AppWindow,
    session: &HerdrSession,
    workspace_id: &str,
    scenario: &Scenario,
    target: u32,
) -> Result<()> {
    let authority = Authority {
        session,
        workspace_id,
    };
    for step in 0..=scenario.tab2_panes.len() {
        if app.test_active_pane_id() == target {
            let wire = scenario
                .pane_map
                .get(&target)
                .context("目标产品 pane 缺 Herdr mapping")?;
            return assert_focus(
                app,
                authority,
                FocusTarget {
                    product_tab: scenario.tab2,
                    product_pane: target,
                    wire_tab: &scenario.wire_tab2,
                    wire_pane: wire,
                },
                &format!("聚焦 pane {target}"),
            );
        }
        let previous = app.test_active_pane_id();
        app.test_handle_action(Action::SwitchPaneNext);
        wait_for(app, "SwitchPaneNext", |candidate| {
            candidate.test_active_pane_id() != previous
        })?;
        let current = app.test_active_pane_id();
        let wire = scenario
            .pane_map
            .get(&current)
            .with_context(|| format!("SwitchPaneNext 到未知 pane {current}"))?;
        assert_focus(
            app,
            authority,
            FocusTarget {
                product_tab: scenario.tab2,
                product_pane: current,
                wire_tab: &scenario.wire_tab2,
                wire_pane: wire,
            },
            &format!("SwitchPaneNext step {step}"),
        )?;
    }
    anyhow::bail!("无法经 SwitchPaneNext 聚焦 pane {target}")
}

fn build_scenario(
    app: &AppWindow,
    session: &HerdrSession,
    workspace_id: &str,
    wire_tab1: String,
    wire_pane1: String,
) -> Result<Scenario> {
    let authority = Authority {
        session,
        workspace_id,
    };
    wait_for(app, "初始 1 tab/1 pane", |candidate| {
        candidate.test_tab_and_pane_counts() == (1, 1)
            && candidate.test_layout_leaf_ids().len() == 1
    })?;
    let tab1 = app.test_active_tab_id();
    let tab1_pane = app.test_active_pane_id();
    assert_focus(
        app,
        authority,
        FocusTarget {
            product_tab: tab1,
            product_pane: tab1_pane,
            wire_tab: &wire_tab1,
            wire_pane: &wire_pane1,
        },
        "初始 attach",
    )?;

    app.test_handle_action(Action::NewTab);
    wait_for(app, "创建 Tab 2", |candidate| {
        candidate.test_tab_ids().len() == 2 && candidate.test_active_tab_id() != tab1
    })?;
    let tab2 = app.test_active_tab_id();
    let left = app.test_active_pane_id();
    let (wire_tab2, wire_left) = server_focus(session, workspace_id)?;

    app.test_handle_action(Action::NewPane);
    wait_for(app, "创建右 pane", |candidate| {
        candidate.test_layout_leaf_ids().len() == 2 && candidate.test_active_pane_id() != left
    })?;
    let right_top = app.test_active_pane_id();
    let (right_tab, wire_right_top) = server_focus(session, workspace_id)?;
    ensure!(right_tab == wire_tab2, "右 split 后 Herdr 切错 tab");

    app.test_handle_action(Action::NewPaneVertical);
    wait_for(app, "创建右下 pane", |candidate| {
        candidate.test_layout_leaf_ids().len() == 3
            && candidate.test_active_pane_id() != right_top
            && candidate.test_gtk_layout_signature() == "H(L,V(L,L))"
            && candidate.test_layout_leaf_ids().iter().all(|pane| {
                let (width, height) = candidate.test_pane_allocation(*pane);
                width > 0 && height > 0
            })
    })?;
    let right_bottom = app.test_active_pane_id();
    let (bottom_tab, wire_right_bottom) = server_focus(session, workspace_id)?;
    ensure!(bottom_tab == wire_tab2, "下 split 后 Herdr 切错 tab");
    ensure!(
        app.test_layout_leaf_ids() == vec![left, right_top, right_bottom],
        "Tab 2 必须严格是 H(left,V(right-top,right-bottom)): {}",
        diagnostics(app)
    );
    ensure!(
        app.test_gtk_paned_orientations()
            == vec![gtk4::Orientation::Horizontal, gtk4::Orientation::Vertical],
        "GtkPaned 方向错误: {:?}",
        app.test_gtk_paned_orientations()
    );

    let pane_map = HashMap::from([
        (tab1_pane, wire_pane1),
        (left, wire_left),
        (right_top, wire_right_top),
        (right_bottom, wire_right_bottom),
    ]);
    ensure!(pane_map.len() == 4, "四个产品 pane id 必须唯一");
    Ok(Scenario {
        tab1,
        tab1_pane,
        wire_tab1,
        tab2,
        tab2_panes: [left, right_top, right_bottom],
        wire_tab2,
        pane_map,
    })
}

fn exercise_all_panes(
    app: &AppWindow,
    session: &HerdrSession,
    workspace_id: &str,
    scenario: &Scenario,
    phase: &str,
) -> Result<Vec<BoundToken>> {
    let authority = Authority {
        session,
        workspace_id,
    };
    switch_tab(
        app,
        Action::SwitchTab1,
        authority,
        FocusTarget {
            product_tab: scenario.tab1,
            product_pane: scenario.tab1_pane,
            wire_tab: &scenario.wire_tab1,
            wire_pane: scenario
                .pane_map
                .get(&scenario.tab1_pane)
                .context("Tab 1 pane 缺 wire mapping")?,
        },
        &format!("{phase} SwitchTab1"),
    )?;
    let mut tokens = vec![execute_and_assert_binding(
        app,
        session,
        &scenario.pane_map,
        &format!("{phase}_T1_P1"),
    )?];

    let initial_tab2_pane = scenario.tab2_panes[2];
    switch_tab(
        app,
        Action::SwitchTab2,
        authority,
        FocusTarget {
            product_tab: scenario.tab2,
            product_pane: initial_tab2_pane,
            wire_tab: &scenario.wire_tab2,
            wire_pane: scenario
                .pane_map
                .get(&initial_tab2_pane)
                .context("Tab 2 active pane 缺 wire mapping")?,
        },
        &format!("{phase} SwitchTab2"),
    )?;
    for (index, pane) in scenario.tab2_panes.iter().copied().enumerate() {
        focus_pane(app, session, workspace_id, scenario, pane)?;
        tokens.push(execute_and_assert_binding(
            app,
            session,
            &scenario.pane_map,
            &format!("{phase}_T2_P{}", index + 1),
        )?);
    }
    focus_pane(app, session, workspace_id, scenario, scenario.tab2_panes[2])?;
    Ok(tokens)
}

fn verify_reattach_continuity(
    app: &AppWindow,
    session: &HerdrSession,
    workspace_id: &str,
    scenario: &Scenario,
    tokens: &[BoundToken],
) -> Result<()> {
    let authority = Authority {
        session,
        workspace_id,
    };
    switch_tab(
        app,
        Action::SwitchTab1,
        authority,
        FocusTarget {
            product_tab: scenario.tab1,
            product_pane: scenario.tab1_pane,
            wire_tab: &scenario.wire_tab1,
            wire_pane: scenario
                .pane_map
                .get(&scenario.tab1_pane)
                .context("Tab 1 pane 缺 wire mapping")?,
        },
        "reattach SwitchTab1",
    )?;
    let working = BoundToken {
        product_pane: scenario.tab1_pane,
        wire_pane: scenario
            .pane_map
            .get(&scenario.tab1_pane)
            .context("Tab 1 pane 缺 wire mapping")?
            .clone(),
        token: "Working...".into(),
    };
    wait_for(app, "reattach 后 agent 当前画面", |candidate| {
        session
            .pane_read_ansi(&working.wire_pane)
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(&working.token))
            && !candidate.test_search_workspace(&working.token).is_empty()
            && candidate
                .test_pane_vte_text(working.product_pane)
                .contains(&working.token)
    })?;
    assert_bound_token(app, session, &scenario.pane_map, &working)?;

    let initial_tab2_pane = scenario.tab2_panes[2];
    switch_tab(
        app,
        Action::SwitchTab2,
        authority,
        FocusTarget {
            product_tab: scenario.tab2,
            product_pane: initial_tab2_pane,
            wire_tab: &scenario.wire_tab2,
            wire_pane: scenario
                .pane_map
                .get(&initial_tab2_pane)
                .context("Tab 2 pane 缺 wire mapping")?,
        },
        "reattach SwitchTab2",
    )?;
    for pane in scenario.tab2_panes {
        focus_pane(app, session, workspace_id, scenario, pane)?;
        let token = tokens
            .iter()
            .find(|token| token.product_pane == pane)
            .with_context(|| format!("缺 pane {pane} pre-agent token"))?;
        assert_bound_token(app, session, &scenario.pane_map, token)?;
    }
    focus_pane(app, session, workspace_id, scenario, scenario.tab2_panes[2])
}

fn wait_agent_detected(session: &HerdrSession, pane_id: &str) -> Result<()> {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        let snapshot = session.snapshot()?;
        if snapshot
            .agents
            .iter()
            .any(|agent| agent.pane_id == pane_id && agent.agent.as_deref() == Some("pi"))
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("Herdr 未识别 pane {pane_id} 的真实 pi")
}

fn wait_detector_working(session: &HerdrSession, pane_id: &str) -> Result<()> {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        let snapshot = session.snapshot()?;
        if snapshot.agents.iter().any(|agent| {
            agent.pane_id == pane_id
                && agent.agent_status == HerdrAgentStatus::Working
                && !agent.screen_detection_skipped
        }) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("释放 hook 后 screen detector 未接管 pane {pane_id}")
}

/// mark_done 后等 server 侧 agent 状态离开 Working（Done/Idle 皆可）。
/// 与 Muxterm 的 done attention 分开等待，失败时可区分是 server 侧
/// detector 未更新，还是事件/attention 链路丢失。
fn wait_for_server_agent_transition(session: &HerdrSession, pane_id: &str) -> Result<()> {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        let snapshot = session.snapshot()?;
        if snapshot.agents.iter().any(|agent| {
            agent.pane_id == pane_id && agent.agent_status != HerdrAgentStatus::Working
        }) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let snapshot = session.snapshot()?;
    anyhow::bail!(
        "server 侧 agent 未离开 Working（mark_done 后 detector 未更新）: {:?}",
        snapshot
            .agents
            .iter()
            .filter(|agent| agent.pane_id == pane_id)
            .collect::<Vec<_>>()
    )
}

fn notification_count(app: &AppWindow, suffix: &str) -> usize {
    app.test_notifications_recorded()
        .iter()
        .filter(|line| line.ends_with(suffix))
        .count()
}

// ─────────────────────────────────────────────────────────────
// 拆解后的独立小测试：每个 #[test] 验证一个独立场景，local/ssh 两个
// transport 共用同一套 setup/断言 helper。失败时打印 herdr server 日志。
// ─────────────────────────────────────────────────────────────

struct AgentTestCtx {
    agent_command: TempAgentCommand,
    // RAII：析构时清理隔离 herdr session（字段不读，但必须存活）。
    #[allow(dead_code)]
    herdr: IsolatedHerdr,
    workspace_id: String,
    wire_pane1: String,
    session: HerdrSession,
    spec: WorkspaceSpec,
    app: AppWindow,
    scenario: Scenario,
    source: String,
    agent_session_path: std::path::PathBuf,
    mark: &'static str,
}

fn transport_mark(transport: &str) -> &'static str {
    match transport {
        "local" => "L",
        "ssh" => "S",
        other => panic!("未知 transport {other}"),
    }
}

/// 独立 setup：herdr server + workspace + GTK app + 四 pane 场景。
fn setup_workspace(sshd: &LoopbackSshd, transport: &str) -> Result<AgentTestCtx> {
    let agent_command = TempAgentCommand::pi(&format!("gtk-agent-{transport}"));
    let herdr = IsolatedHerdr::start(&format!("gtk-agent-{transport}"));
    let (workspace_id, wire_tab1, wire_pane1) = herdr.create_workspace(
        agent_command
            .cwd()
            .to_str()
            .context("临时 agent cwd 不是 UTF-8")?,
        &format!("mux-agent-gtk-{transport}"),
    );
    let session = HerdrSession::new(herdr.name(), herdr.socket_path());
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
    let app = AppWindow::new(Config::default(), load_theme());
    app.window.set_default_size(1280, 800);
    app.window.present();
    gtk4::test_widget_wait_for_draw(&app.window);
    pump_main_loop(100);
    app.test_open_spec(spec.clone());
    let scenario = build_scenario(
        &app,
        &session,
        &workspace_id,
        wire_tab1.clone(),
        wire_pane1.clone(),
    )?;
    let source = format!("herdr:pi:{transport}");
    let agent_session_path = agent_command.cwd().join("pi-session.jsonl");
    Ok(AgentTestCtx {
        agent_command,
        herdr,
        workspace_id,
        wire_pane1,
        session,
        spec,
        app,
        scenario,
        source,
        agent_session_path,
        mark: transport_mark(transport),
    })
}

/// 在 Tab 1 启动可检测 pi 并 report working，随后切回 Tab 2 后台化。
fn start_agent(ctx: &AgentTestCtx) -> Result<()> {
    let authority = Authority {
        session: &ctx.session,
        workspace_id: &ctx.workspace_id,
    };
    switch_tab(
        &ctx.app,
        Action::SwitchTab1,
        authority,
        FocusTarget {
            product_tab: ctx.scenario.tab1,
            product_pane: ctx.scenario.tab1_pane,
            wire_tab: &ctx.scenario.wire_tab1,
            wire_pane: ctx
                .scenario
                .pane_map
                .get(&ctx.scenario.tab1_pane)
                .context("agent pane 缺 wire mapping")?,
        },
        "启动 agent 前切 Tab 1",
    )?;
    emit_command(&ctx.app, ctx.agent_command.invocation())?;
    wait_agent_detected(&ctx.session, &ctx.wire_pane1)?;
    std::fs::write(&ctx.agent_session_path, "{}\n").context("创建临时 pi session")?;
    ctx.session.call(
        "pane.report_agent_session",
        serde_json::json!({
            "pane_id": &ctx.wire_pane1,
            "source": &ctx.source,
            "agent": "pi",
            "seq": 1,
            "agent_session_path": &ctx.agent_session_path,
            "session_start_source": "startup",
        }),
    )?;
    ctx.session.call(
        "pane.report_agent",
        serde_json::json!({
            "pane_id": &ctx.wire_pane1,
            "source": &ctx.source,
            "agent": "pi",
            "state": "working",
            "message": format!("running GTK lifecycle"),
            "seq": 2,
            "agent_session_path": &ctx.agent_session_path,
        }),
    )?;
    switch_tab(
        &ctx.app,
        Action::SwitchTab2,
        authority,
        FocusTarget {
            product_tab: ctx.scenario.tab2,
            product_pane: ctx.scenario.tab2_panes[2],
            wire_tab: &ctx.scenario.wire_tab2,
            wire_pane: ctx
                .scenario
                .pane_map
                .get(&ctx.scenario.tab2_panes[2])
                .context("Tab 2 active pane 缺 wire mapping")?,
        },
        "agent 后台化",
    )?;
    Ok(())
}

/// 跑一个场景：setup + body；失败时打印 herdr server 日志。
fn run_agent_test(
    sshd: &LoopbackSshd,
    transport: &str,
    body: fn(&AgentTestCtx) -> Result<()>,
) -> Result<()> {
    let ctx = setup_workspace(sshd, transport)?;
    let result = body(&ctx);
    if result.is_err() {
        eprintln!("herdr-server.log tail:\n{}", server_log_tail(&ctx.session));
    }
    ctx.app.shutdown();
    result
}

/// local + ssh 两个 transport 各跑一次场景。
fn run_agent_test_pair(name: &'static str, body: fn(&AgentTestCtx) -> Result<()>) {
    if skip_no_display() {
        return;
    }
    // 仅在 RUST_LOG 明确设置时输出 Muxterm tracing（CI 不设则不输出）。
    if std::env::var("RUST_LOG").is_ok() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .try_init();
    }
    assert!(herdr_available(), "GTK Herdr agent e2e 要求 herdr");
    assert!(
        loopback_sshd_available(),
        "GTK Herdr agent e2e 要求可自启 loopback sshd"
    );
    gtk4::test_synced(move || {
        gtk_test_framework_smoke();
        let sshd = LoopbackSshd::start("gtk-herdr-agent").expect("启动 loopback sshd");
        let _ssh_config = EnvRestore::set("MUXTERM_SSH_CONFIG_PATH", &sshd.config_path);
        for transport in ["local", "ssh"] {
            run_agent_test(&sshd, transport, body)
                .unwrap_or_else(|error| panic!("GTK Herdr agent {transport} {name}: {error:#}"));
        }
    });
}

/// 场景 1：初始 attach 后，四个 pane 都能执行命令并绑定 token。
#[test]
fn herdr_agent_initial_attach_binds_tokens() {
    run_agent_test_pair("initial attach", |ctx| {
        let tokens = exercise_all_panes(
            &ctx.app,
            &ctx.session,
            &ctx.workspace_id,
            &ctx.scenario,
            &format!("B{}", ctx.mark),
        )?;
        ensure!(
            tokens
                .iter()
                .map(|token| token.product_pane)
                .collect::<HashSet<_>>()
                == ctx
                    .scenario
                    .pane_map
                    .keys()
                    .copied()
                    .collect::<HashSet<_>>(),
            "初始 attach 必须绑定全部四个 pane"
        );
        Ok(())
    });
}

/// 场景 2：detach + reopen 后，working 作为 bootstrap 恢复，四 pane 内容连续。
#[test]
fn herdr_agent_detach_reattach_preserves_content() {
    run_agent_test_pair("detach/reattach", |ctx| {
        let before_tokens = exercise_all_panes(
            &ctx.app,
            &ctx.session,
            &ctx.workspace_id,
            &ctx.scenario,
            &format!("B{}", ctx.mark),
        )?;
        // 阶段快照（诊断 reopen 内容丢失用；成功时不打印）。
        let pane2_wire = ctx.scenario.pane_map[&ctx.scenario.tab2_panes[0]].clone();
        let snap_b = ctx
            .session
            .pane_read_recent_ansi_lines(&pane2_wire, 2000)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_else(|e| format!("read err: {e}"));
        start_agent(ctx)?;
        ensure!(
            String::from_utf8_lossy(&ctx.session.pane_read_ansi(&ctx.wire_pane1)?)
                .contains("Working..."),
            "detach 前 agent pane 服务端缺 Working..."
        );
        ensure!(
            ctx.app
                .test_active_runtime_supports(RuntimeCapability::PersistDetach),
            "Herdr 必须声明 PersistDetach"
        );
        ensure!(
            ctx.app.test_detach_active_workspace_outcome()? == TaskOutcome::Done,
            "Herdr detach 必须成功"
        );
        for token in before_tokens
            .iter()
            .filter(|token| ctx.scenario.tab2_panes.contains(&token.product_pane))
        {
            ensure!(
                String::from_utf8_lossy(&ctx.session.pane_read_ansi(&token.wire_pane)?)
                    .contains(&token.token),
                "detach 后、reopen 前服务端 {} 丢失 {}",
                token.wire_pane,
                token.token
            );
        }
        ctx.app.test_open_spec(ctx.spec.clone());
        wait_for(&ctx.app, "agent working reattach", |candidate| {
            candidate.test_active_workspace_runtime() == "herdr"
                && candidate.test_tab_ids().len() == 2
                && candidate.test_active_tab_id() == ctx.scenario.tab2
                && candidate.test_layout_leaf_ids() == ctx.scenario.tab2_panes
        })?;
        let reattach = verify_reattach_continuity(
            &ctx.app,
            &ctx.session,
            &ctx.workspace_id,
            &ctx.scenario,
            &before_tokens,
        );
        if let Err(err) = reattach {
            let snap_now = ctx
                .session
                .pane_read_recent_ansi_lines(&pane2_wire, 2000)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_else(|e| format!("read err: {e}"));
            eprintln!(
                "detach/reattach 阶段快照（pane {pane2_wire}）\nB 阶段后: {:?}\nreopen 后: {:?}",
                snap_b.escape_debug(),
                snap_now.escape_debug()
            );
            return Err(err.context("reattach 后四 pane 内容连续性"));
        }
        for _ in 0..10 {
            tick(&ctx.app);
        }
        ensure!(
            ctx.app.test_attention_blocked_workspaces() == 0
                && ctx.app.test_attention_done_count() == 0
                && notification_count(&ctx.app, ": needs attention") == 0
                && notification_count(&ctx.app, ": task complete") == 0,
            "attach 的初始 working 不得伪造通知: {:?}",
            ctx.app.test_notifications_recorded()
        );
        Ok(())
    });
}

/// 场景 3：后台 agent 报 blocked 时产生一次 needs-attention 通知。
#[test]
fn herdr_agent_blocked_attention() {
    run_agent_test_pair("blocked attention", |ctx| {
        let _ = exercise_all_panes(
            &ctx.app,
            &ctx.session,
            &ctx.workspace_id,
            &ctx.scenario,
            &format!("B{}", ctx.mark),
        )?;
        start_agent(ctx)?;
        ctx.session.call(
            "pane.report_agent",
            serde_json::json!({
                "pane_id": &ctx.wire_pane1,
                "source": &ctx.source,
                "agent": "pi",
                "state": "blocked",
                "message": format!("approve GTK command"),
                "seq": 3,
                "agent_session_path": &ctx.agent_session_path,
            }),
        )?;
        wait_for(&ctx.app, "blocked attention", |candidate| {
            candidate.test_attention_blocked_workspaces() == 1
                && notification_count(candidate, ": needs attention") == 1
        })?;
        for _ in 0..10 {
            tick(&ctx.app);
        }
        ensure!(
            notification_count(&ctx.app, ": needs attention") == 1,
            "blocked 保持期间只能通知一次: {:?}",
            ctx.app.test_notifications_recorded()
        );
        Ok(())
    });
}

/// 场景 4：释放 authority 后由 screen detector 接管，真实 pi 完成帧触发
/// 一次 task-complete 通知。
#[test]
fn herdr_agent_done_attention() {
    run_agent_test_pair("done attention", |ctx| {
        let _ = exercise_all_panes(
            &ctx.app,
            &ctx.session,
            &ctx.workspace_id,
            &ctx.scenario,
            &format!("B{}", ctx.mark),
        )?;
        start_agent(ctx)?;
        ctx.session.call(
            "pane.clear_agent_authority",
            serde_json::json!({
                "pane_id": &ctx.wire_pane1,
                "source": &ctx.source,
                "seq": 4,
            }),
        )?;
        wait_detector_working(&ctx.session, &ctx.wire_pane1)?;
        ctx.agent_command.mark_done();
        // 先确认 server 侧 detector 已从 Working 转走（Done/Idle），
        // 再等 Muxterm 的 done attention——失败时可区分是 server 未报
        // 还是事件/attention 链路丢失。
        wait_for_server_agent_transition(&ctx.session, &ctx.wire_pane1)?;
        wait_for(&ctx.app, "done attention", |candidate| {
            candidate.test_attention_done_count() == 1
                && notification_count(candidate, ": task complete") == 1
        })?;
        for _ in 0..10 {
            tick(&ctx.app);
        }
        ensure!(
            notification_count(&ctx.app, ": task complete") == 1,
            "done 保持期间只能通知一次: {:?}",
            ctx.app.test_notifications_recorded()
        );
        Ok(())
    });
}

/// 场景 5：完整 agent 生命周期（working/blocked/done）结束后，四个 pane
/// 仍能执行命令——authority/observe/control 切换不得破坏终端。
#[test]
fn herdr_agent_lifecycle_after_commands() {
    run_agent_test_pair("lifecycle after commands", |ctx| {
        let before_tokens = exercise_all_panes(
            &ctx.app,
            &ctx.session,
            &ctx.workspace_id,
            &ctx.scenario,
            &format!("B{}", ctx.mark),
        )?;
        start_agent(ctx)?;
        ctx.session.call(
            "pane.report_agent",
            serde_json::json!({
                "pane_id": &ctx.wire_pane1,
                "source": &ctx.source,
                "agent": "pi",
                "state": "blocked",
                "message": format!("approve GTK command"),
                "seq": 3,
                "agent_session_path": &ctx.agent_session_path,
            }),
        )?;
        wait_for(&ctx.app, "blocked attention", |candidate| {
            candidate.test_attention_blocked_workspaces() == 1
                && notification_count(candidate, ": needs attention") == 1
        })?;
        ctx.session.call(
            "pane.clear_agent_authority",
            serde_json::json!({
                "pane_id": &ctx.wire_pane1,
                "source": &ctx.source,
                "seq": 4,
            }),
        )?;
        wait_detector_working(&ctx.session, &ctx.wire_pane1)?;
        ctx.agent_command.mark_done();
        // 与 done_attention 场景一致：先确认 server 侧 detector 已从 Working
        // 转走（Done/Idle），再等产品侧 done attention——ssh 下事件链路
        // 可能滞后于 server 状态，直接等产品侧会偶发超时。
        wait_for_server_agent_transition(&ctx.session, &ctx.wire_pane1)?;
        wait_for(&ctx.app, "done attention", |candidate| {
            candidate.test_attention_done_count() == 1
                && notification_count(candidate, ": task complete") == 1
        })?;
        ctx.agent_command.stop();
        let authority = Authority {
            session: &ctx.session,
            workspace_id: &ctx.workspace_id,
        };
        switch_tab(
            &ctx.app,
            Action::SwitchTab1,
            authority,
            FocusTarget {
                product_tab: ctx.scenario.tab1,
                product_pane: ctx.scenario.tab1_pane,
                wire_tab: &ctx.scenario.wire_tab1,
                wire_pane: ctx
                    .scenario
                    .pane_map
                    .get(&ctx.scenario.tab1_pane)
                    .context("agent pane 缺 wire mapping")?,
            },
            "完成后切回 agent pane",
        )?;
        wait_for(&ctx.app, "聚焦后 attention 清零", |candidate| {
            candidate.test_attention_blocked_workspaces() == 0
                && candidate.test_attention_done_count() == 0
        })?;
        let after_tokens = exercise_all_panes(
            &ctx.app,
            &ctx.session,
            &ctx.workspace_id,
            &ctx.scenario,
            &format!("A{}", ctx.mark),
        )?;
        ensure!(
            after_tokens
                .iter()
                .map(|token| token.product_pane)
                .collect::<HashSet<_>>()
                == ctx
                    .scenario
                    .pane_map
                    .keys()
                    .copied()
                    .collect::<HashSet<_>>(),
            "agent lifecycle 后必须再次在全部四个 pane 成功执行命令"
        );
        let _ = before_tokens;
        Ok(())
    });
}
