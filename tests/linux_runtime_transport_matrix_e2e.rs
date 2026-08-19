//! GTK/VTE 的完整 Runtime × Transport 行为矩阵。
//!
//! 同一个 `AppWindow` 依次验证生产快捷动作、真实 VTE commit、GTK split、
//! WorkspacePool 切换，以及声明 `PersistDetach` 的 Runtime 重连。专用门禁应以
//! `G_DEBUG=fatal-criticals xvfb-run -a ... --test-threads=1` 运行。

#![cfg(feature = "gtk")]

mod support;

use std::any::Any;
use std::collections::HashSet;
use std::ffi::OsString;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use gtk4::prelude::*;

use muxterm::core::catalog::Catalog;
use muxterm::core::config::{Action, Config};
use muxterm::core::model::backend::RuntimeCapability;
use muxterm::core::model::task::TaskOutcome;
use muxterm::platform::linux::window::AppWindow;

use support::herdr_test_support::herdr_available;
use support::linux_gtk::{gtk_test_framework_smoke, load_theme, pump_main_loop, skip_no_display};
use support::runtime_transport_matrix::MatrixFixture;
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};
use support::tmux_test_support::tmux_available;

const GTK_MATRIX_TIMEOUT: Duration = Duration::from_secs(15);

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
struct GuiSnapshot {
    tab1: u32,
    tab1_pane: u32,
    tab2: u32,
    tab2_panes: Vec<u32>,
    active_pane: u32,
    tokens: Vec<(u32, String)>,
}

fn panic_text(payload: Box<dyn Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else {
        "non-string panic".into()
    }
}

fn matrix_label(runtime: &str, transport: &str) -> String {
    let runtime = match runtime {
        "tmux" => "TM",
        "herdr" => "HD",
        "shell" => "SH",
        _ => "XX",
    };
    let transport = match transport {
        "local" => "L",
        "ssh" => "S",
        _ => "X",
    };
    format!("{runtime}{transport}")
}

fn tick(app: &AppWindow) {
    app.test_poll_once();
    pump_main_loop(25);
    app.test_flush_feeds();
}

fn diagnostics(app: &AppWindow) -> String {
    let leaves = app.test_layout_leaf_ids();
    let vte = leaves
        .iter()
        .map(|pane| {
            let text = app.test_pane_vte_text(*pane);
            let tail = text
                .chars()
                .rev()
                .take(240)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>();
            (*pane, tail.escape_debug().to_string())
        })
        .collect::<Vec<_>>();
    format!(
        "workspace={} runtime={} tabs/panes={:?} active_tab={} active_pane={} leaves={leaves:?} gtk={} vte={vte:?}",
        app.test_active_workspace_replica_id(),
        app.test_active_workspace_runtime(),
        app.test_tab_and_pane_counts(),
        app.test_active_tab_id(),
        app.test_active_pane_id(),
        app.test_gtk_layout_signature(),
    )
}

fn wait_for(
    app: &AppWindow,
    label: &str,
    mut predicate: impl FnMut(&AppWindow) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + GTK_MATRIX_TIMEOUT;
    while Instant::now() < deadline {
        tick(app);
        if predicate(app) {
            return Ok(());
        }
    }
    anyhow::bail!("{label} 超时: {}", diagnostics(app))
}

fn wait_active_pane_change(app: &AppWindow, previous: u32, label: &str) -> Result<u32> {
    wait_for(app, label, |app| app.test_active_pane_id() != previous)?;
    Ok(app.test_active_pane_id())
}

fn assert_three_pane_surface(app: &AppWindow, label: &str) -> Result<Vec<u32>> {
    wait_for(app, label, |app| {
        let leaves = app.test_layout_leaf_ids();
        leaves.len() == 3
            && app.test_gtk_layout_signature() == "H(L,V(L,L))"
            && leaves.iter().all(|pane| {
                let (width, height) = app.test_pane_allocation(*pane);
                width > 0 && height > 0
            })
    })?;
    let leaves = app.test_layout_leaf_ids();
    ensure!(
        leaves.iter().copied().collect::<HashSet<_>>().len() == 3,
        "{label}: 三个 leaf 必须是三个唯一 VTE: {leaves:?}"
    );
    ensure!(
        app.test_gtk_paned_orientations()
            == vec![gtk4::Orientation::Horizontal, gtk4::Orientation::Vertical],
        "{label}: GtkPaned 必须严格是 [Horizontal, Vertical]，实际 {:?}",
        app.test_gtk_paned_orientations()
    );
    ensure!(
        app.test_gtk_layout_signature() == "H(L,V(L,L))",
        "{label}: GTK 必须是 H(left,V(right-top,right-bottom))，实际 {}",
        app.test_gtk_layout_signature()
    );
    Ok(leaves)
}

fn emit_command(app: &AppWindow, command: &str) -> Result<()> {
    for character in command.chars() {
        ensure!(
            app.test_emit_active_pane_commit(&character.to_string()),
            "VTE commit 找不到 active PaneView: {}",
            diagnostics(app)
        );
    }
    ensure!(
        app.test_emit_active_pane_commit("\r"),
        "VTE Enter 找不到 active PaneView"
    );
    Ok(())
}

fn execute_printf(
    app: &AppWindow,
    suffix: &str,
    require_ssh_connection: bool,
) -> Result<(u32, String)> {
    let pane = app.test_active_pane_id();
    let token = format!("MX_{suffix}");
    let command = if require_ssh_connection {
        format!("if [ -n \"$SSH_CONNECTION\" ]; then printf 'MX_%s\\n' '{suffix}'; fi")
    } else {
        format!("printf 'MX_%s\\n' '{suffix}'")
    };
    ensure!(
        !command.contains(&token),
        "输入命令不得原样包含期望输出 token {token}"
    );
    ensure!(
        app.test_search_workspace(&token).is_empty(),
        "token {token} 在执行前就已存在"
    );
    emit_command(app, &command)?;
    wait_for(app, &format!("pane {pane} 输出 {token}"), |app| {
        let hits = app.test_search_workspace(&token);
        app.test_pane_vte_text(pane).contains(&token)
            && hits.iter().any(|(_, hit_pane, _)| *hit_pane == pane)
    })?;
    let hit_panes = app
        .test_search_workspace(&token)
        .into_iter()
        .map(|(_, pane, _)| pane)
        .collect::<HashSet<_>>();
    ensure!(
        hit_panes == HashSet::from([pane]),
        "token {token} 的 Workspace 搜索归属错误: {hit_panes:?}"
    );
    Ok((pane, token))
}

fn focus_pane_with_actions(app: &AppWindow, target: u32, panes: &[u32]) -> Result<()> {
    for _ in 0..=panes.len() {
        if app.test_active_pane_id() == target {
            return Ok(());
        }
        let before = app.test_active_pane_id();
        app.test_handle_action(Action::SwitchPaneNext);
        let _ = wait_active_pane_change(app, before, "SwitchPaneNext")?;
    }
    anyhow::bail!(
        "无法经 SwitchPaneNext 聚焦 pane {target}: {}",
        diagnostics(app)
    )
}

fn exercise_three_panes(
    app: &AppWindow,
    panes: &[u32],
    token_prefix: &str,
) -> Result<Vec<(u32, String)>> {
    let mut seen = HashSet::new();
    let mut tokens = Vec::new();
    for index in 0..panes.len() {
        let pane = app.test_active_pane_id();
        ensure!(panes.contains(&pane), "active pane {pane} 不在 {panes:?}");
        ensure!(seen.insert(pane), "SwitchPaneNext 提前循环到 pane {pane}");
        tokens.push(execute_printf(
            app,
            &format!("{token_prefix}_P{}", index + 1),
            false,
        )?);
        if index + 1 < panes.len() {
            app.test_handle_action(Action::SwitchPaneNext);
            let _ = wait_active_pane_change(app, pane, "SwitchPaneNext 切换三 pane")?;
        }
    }
    ensure!(
        seen == panes.iter().copied().collect::<HashSet<_>>(),
        "SwitchPaneNext 必须遍历全部三 pane: seen={seen:?}, panes={panes:?}"
    );

    let before = app.test_active_pane_id();
    app.test_handle_action(Action::SwitchPanePrev);
    let previous = wait_active_pane_change(app, before, "SwitchPanePrev")?;
    ensure!(
        panes.contains(&previous),
        "SwitchPanePrev 落到未知 pane {previous}"
    );
    Ok(tokens)
}

fn assert_search_ownership(app: &AppWindow, tokens: &[(u32, String)]) -> Result<()> {
    for (expected_pane, token) in tokens {
        let search_panes = app
            .test_search_workspace(token)
            .into_iter()
            .map(|(_, pane, _)| pane)
            .collect::<HashSet<_>>();
        ensure!(
            search_panes == HashSet::from([*expected_pane]),
            "token {token} 必须只在 PaneBuf pane {expected_pane}，实际 {search_panes:?}"
        );
    }
    Ok(())
}

fn wait_visible_vte_ownership(
    app: &AppWindow,
    visible_panes: &[u32],
    tokens: &[(u32, String)],
) -> Result<()> {
    for (expected_pane, token) in tokens
        .iter()
        .filter(|(pane, _)| visible_panes.contains(pane))
    {
        wait_for(
            app,
            &format!("token {token} 只在可见 VTE pane {expected_pane}"),
            |app| {
                visible_panes
                    .iter()
                    .copied()
                    .filter(|pane| app.test_pane_vte_text(*pane).contains(token))
                    .collect::<Vec<_>>()
                    == vec![*expected_pane]
            },
        )?;
    }
    Ok(())
}

fn build_gui_scenario(app: &AppWindow, runtime: &str, transport: &str) -> Result<GuiSnapshot> {
    let pair = matrix_label(runtime, transport);
    wait_for(app, "初始 1 tab / 1 pane", |app| {
        app.test_tab_and_pane_counts() == (1, 1)
            && app
                .test_layout_leaf_ids()
                .contains(&app.test_active_pane_id())
    })?;
    let tab1 = app.test_active_tab_id();
    let tab1_pane = app.test_active_pane_id();

    app.test_handle_action(Action::NewTab);
    wait_for(app, "生产 NewTab 创建 Tab 2", |app| {
        app.test_tab_ids().len() == 2
            && app.test_active_tab_id() != tab1
            && app.test_tab_and_pane_counts() == (2, 1)
            && app
                .test_layout_leaf_ids()
                .contains(&app.test_active_pane_id())
    })?;
    let tab2 = app.test_active_tab_id();

    app.test_handle_action(Action::NewPane);
    wait_for(app, "生产 NewPane 创建右 pane", |app| {
        app.test_layout_leaf_ids().len() == 2
    })?;
    app.test_handle_action(Action::NewPaneVertical);
    let tab2_panes = assert_three_pane_surface(app, "生产 NewPaneVertical")?;
    ensure!(
        app.test_active_pane_id() == tab2_panes[2],
        "第二次 split 后必须聚焦右下 pane: {}",
        diagnostics(app)
    );

    app.test_handle_action(Action::SwitchTab1);
    wait_for(app, "生产 SwitchTab1", |app| {
        app.test_active_tab_id() == tab1 && app.test_layout_leaf_ids() == vec![tab1_pane]
    })?;
    let mut tokens = vec![execute_printf(
        app,
        &format!("GTK_{pair}_T1_P1"),
        runtime == "shell" && transport == "ssh",
    )?];

    app.test_handle_action(Action::SwitchTab2);
    wait_for(app, "生产 SwitchTab2", |app| {
        app.test_active_tab_id() == tab2 && app.test_layout_leaf_ids().len() == 3
    })?;
    let current_panes = assert_three_pane_surface(app, "切回 Tab 2")?;
    ensure!(current_panes == tab2_panes, "Tab 2 leaf 顺序切换后改变");
    tokens.extend(exercise_three_panes(
        app,
        &tab2_panes,
        &format!("GTK_{pair}_T2"),
    )?);
    assert_search_ownership(app, &tokens)?;
    wait_visible_vte_ownership(app, &tab2_panes, &tokens)?;

    app.test_handle_action(Action::SwitchTab1);
    wait_for(app, "写入后再次切回 Tab 1", |app| {
        app.test_active_tab_id() == tab1 && app.test_layout_leaf_ids() == vec![tab1_pane]
    })?;
    wait_visible_vte_ownership(app, &[tab1_pane], &tokens)?;
    app.test_handle_action(Action::SwitchTab2);
    wait_for(app, "写入后再次切回 Tab 2", |app| {
        app.test_active_tab_id() == tab2 && app.test_layout_leaf_ids() == tab2_panes
    })?;
    let _ = assert_three_pane_surface(app, "写入后切回 Tab 2")?;
    wait_visible_vte_ownership(app, &tab2_panes, &tokens)?;
    focus_pane_with_actions(app, tab2_panes[2], &tab2_panes)?;

    Ok(GuiSnapshot {
        tab1,
        tab1_pane,
        tab2,
        tab2_panes,
        active_pane: app.test_active_pane_id(),
        tokens,
    })
}

fn verify_pool_switch(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
    original_replica: &str,
    snapshot: &GuiSnapshot,
) -> Result<(u32, String)> {
    let alternate_replica = fixture.alternate_spec.id().replica_id();
    app.test_open_spec(fixture.alternate_spec.clone());
    wait_for(app, "第二 Workspace 激活", |app| {
        app.test_active_workspace_replica_id() == alternate_replica
            && app.test_active_workspace_runtime() == runtime
            && app.test_tab_and_pane_counts() == (1, 1)
    })?;
    let (_, alternate_token) = execute_printf(
        app,
        &format!("GTK_{}_POOL_ALT", matrix_label(runtime, transport)),
        runtime == "shell" && transport == "ssh",
    )?;
    ensure!(
        app.test_workspace_replica_ids()
            .iter()
            .any(|id| id == original_replica),
        "创建第二 Workspace 后原 Workspace 必须仍在池里"
    );

    app.test_activate_workspace(original_replica);
    wait_for(app, "WorkspacePool::activate 切回原 Workspace", |app| {
        app.test_active_workspace_replica_id() == original_replica
            && app.test_active_tab_id() == snapshot.tab2
            && app.test_active_pane_id() == snapshot.active_pane
            && app.test_layout_leaf_ids() == snapshot.tab2_panes
    })?;
    let _ = assert_three_pane_surface(app, "WorkspacePool 切回后 GTK layout")?;
    assert_search_ownership(app, &snapshot.tokens)?;
    wait_visible_vte_ownership(app, &snapshot.tab2_panes, &snapshot.tokens)?;
    app.test_handle_action(Action::SwitchTab1);
    wait_for(app, "WorkspacePool 切回后显示 Tab 1", |app| {
        app.test_active_tab_id() == snapshot.tab1
            && app.test_layout_leaf_ids() == vec![snapshot.tab1_pane]
    })?;
    wait_visible_vte_ownership(app, &[snapshot.tab1_pane], &snapshot.tokens)?;
    app.test_handle_action(Action::SwitchTab2);
    wait_for(app, "WorkspacePool 切回后显示 Tab 2", |app| {
        app.test_active_tab_id() == snapshot.tab2
            && app.test_layout_leaf_ids() == snapshot.tab2_panes
    })?;
    let _ = assert_three_pane_surface(app, "WorkspacePool 切回后重新显示 Tab 2")?;
    wait_visible_vte_ownership(app, &snapshot.tab2_panes, &snapshot.tokens)?;
    focus_pane_with_actions(app, snapshot.active_pane, &snapshot.tab2_panes)?;
    ensure!(
        app.test_search_workspace(&alternate_token).is_empty(),
        "第二 Workspace token 不得串入原 Workspace"
    );
    ensure!(
        !app.test_search_all(&alternate_token).is_empty(),
        "第二 Workspace 在后台时 token 必须仍可被 WorkspacePool::search_all 找到"
    );
    let return_token = execute_printf(
        app,
        &format!("GTK_{}_POOL_RETURN", matrix_label(runtime, transport)),
        false,
    )?;
    Ok(return_token)
}

fn verify_supported_reattach(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
    snapshot: &GuiSnapshot,
    latest_visible_tokens: &[(u32, String)],
) -> Result<()> {
    let persistent = app.test_active_runtime_supports(RuntimeCapability::PersistDetach);
    if !persistent {
        ensure!(
            runtime == "shell",
            "没有 PersistDetach 的内置 Runtime 应只有 shell，实际 {runtime}"
        );
        let workspace_before = app.test_active_workspace_replica_id();
        let pane_before = app.test_active_pane_id();
        let outcome = app.test_detach_active_workspace_outcome()?;
        ensure!(
            matches!(outcome, TaskOutcome::Rejected { .. }),
            "shell 必须明确拒绝 detach，不能跳过或伪造成功，实际 {outcome:?}"
        );
        ensure!(
            app.test_active_workspace_replica_id() == workspace_before
                && app
                    .test_workspace_replica_ids()
                    .iter()
                    .any(|id| id == &workspace_before),
            "shell detach 被拒绝后原 Workspace 必须继续留在 WorkspacePool"
        );
        let continued = execute_printf(
            app,
            &format!(
                "GTK_{}_DETACH_REJECTED_CONTINUES",
                matrix_label(runtime, transport)
            ),
            transport == "ssh",
        )?;
        ensure!(
            continued.0 == pane_before,
            "shell detach 被拒绝后输入必须继续落在原 active pane: before={pane_before}, after={}",
            continued.0
        );
        return Ok(());
    }

    ensure!(
        app.test_detach_active_workspace_outcome()? == TaskOutcome::Done,
        "声明 PersistDetach 后 Task::Detach 必须成功"
    );
    app.test_open_spec(fixture.spec.clone());
    wait_for(app, "新 Runtime reattach 恢复 2 tab / 3 pane", |app| {
        app.test_active_workspace_replica_id() == fixture.spec.id().replica_id()
            && app.test_active_workspace_runtime() == runtime
            && app.test_tab_ids().len() == 2
            && app.test_active_tab_id() == snapshot.tab2
            && app.test_layout_leaf_ids().len() == 3
    })?;
    let panes = assert_three_pane_surface(app, "reattach 后 GTK layout")?;
    ensure!(
        panes == snapshot.tab2_panes,
        "reattach 后 pane id/layout 必须稳定: before={:?}, after={panes:?}",
        snapshot.tab2_panes
    );
    for (_, token) in &snapshot.tokens {
        ensure!(
            !app.test_search_workspace(token).is_empty(),
            "reattach 后新 Runtime 的 PaneBuf 必须恢复旧 token {token}"
        );
    }

    app.test_handle_action(Action::SwitchTab1);
    wait_for(app, "reattach 后 SwitchTab1", |app| {
        app.test_active_tab_id() == snapshot.tab1
            && app.test_layout_leaf_ids() == vec![snapshot.tab1_pane]
    })?;
    wait_visible_vte_ownership(app, &[snapshot.tab1_pane], latest_visible_tokens)?;
    let mut tokens = vec![execute_printf(
        app,
        &format!("GTK_{}_REATTACH_T1", matrix_label(runtime, transport)),
        false,
    )?];

    app.test_handle_action(Action::SwitchTab2);
    wait_for(app, "reattach 后 SwitchTab2", |app| {
        app.test_active_tab_id() == snapshot.tab2 && app.test_layout_leaf_ids().len() == 3
    })?;
    let panes = assert_three_pane_surface(app, "reattach 后切回 Tab 2")?;
    wait_visible_vte_ownership(app, &panes, latest_visible_tokens)?;
    tokens.extend(exercise_three_panes(
        app,
        &panes,
        &format!("GTK_{}_REATTACH_T2", matrix_label(runtime, transport)),
    )?);
    assert_search_ownership(app, &tokens)?;
    wait_visible_vte_ownership(app, &panes, &tokens)?;
    Ok(())
}

fn run_case(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    eprintln!("GTK_MATRIX {runtime} x {transport}: open primary");
    let original_replica = fixture.spec.id().replica_id();
    app.test_open_spec(fixture.spec.clone());
    wait_for(app, "主 Workspace 激活", |app| {
        app.test_active_workspace_replica_id() == original_replica
            && app.test_active_workspace_runtime() == runtime
    })?;
    let snapshot = build_gui_scenario(app, runtime, transport)
        .with_context(|| format!("{runtime} x {transport} 构造 GTK 2tab3pane"))?;
    eprintln!("GTK_MATRIX {runtime} x {transport}: pool switch");
    let return_token = verify_pool_switch(
        app,
        fixture,
        runtime,
        transport,
        &original_replica,
        &snapshot,
    )?;
    let mut latest_visible_tokens = snapshot.tokens.clone();
    latest_visible_tokens.retain(|(pane, _)| *pane != return_token.0);
    latest_visible_tokens.push(return_token.clone());
    ensure!(
        app.test_pane_vte_text(return_token.0)
            .contains(&return_token.1),
        "WorkspacePool 切回后输入输出必须落在保留的 active pane"
    );
    eprintln!("GTK_MATRIX {runtime} x {transport}: supported reattach");
    verify_supported_reattach(
        app,
        fixture,
        runtime,
        transport,
        &snapshot,
        &latest_visible_tokens,
    )?;
    eprintln!("GTK_MATRIX {runtime} x {transport}: ok");
    Ok(())
}

#[test]
fn linux_every_registered_runtime_transport_passes_gui_input_pool_and_reattach_matrix() {
    if skip_no_display() {
        return;
    }
    assert!(tmux_available(), "GTK 六格矩阵要求 tmux fixture 可用");
    assert!(herdr_available(), "GTK 六格矩阵要求 Herdr fixture 可用");
    assert!(
        loopback_sshd_available(),
        "GTK 六格矩阵要求可自启的 loopback sshd"
    );

    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let sshd = LoopbackSshd::start("gtk-runtime-matrix").expect("启动隔离 loopback sshd");
        let _ssh_config = EnvRestore::set("MUXTERM_SSH_CONFIG_PATH", &sshd.config_path);

        let registry = Catalog::with_builtins();
        let runtimes = registry.runtime_list();
        let transports = registry.transport_list();
        let expected_cases = runtimes.len() * transports.len();
        let mut cfg = Config::default();
        cfg.pool.max_slots = (expected_cases * 2 + 4) as u32;
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        let mut failures = Vec::new();
        let mut executed = 0usize;
        let mut fixtures = Vec::new();
        for runtime in &runtimes {
            for transport in &transports {
                executed += 1;
                if !runtime.accepted_transports.contains(&transport.id) {
                    failures.push(format!(
                        "{} x {}: RuntimeInfo.accepted_transports 未声明该组合",
                        runtime.id, transport.id
                    ));
                    continue;
                }
                let fixture = match catch_unwind(AssertUnwindSafe(|| {
                    MatrixFixture::new(&runtime.id, &transport.id, &sshd)
                })) {
                    Ok(Ok(fixture)) => fixture,
                    Ok(Err(error)) => {
                        failures.push(format!(
                            "{} x {} fixture: {error:#}",
                            runtime.id, transport.id
                        ));
                        continue;
                    }
                    Err(payload) => {
                        failures.push(format!(
                            "{} x {} fixture panic: {}",
                            runtime.id,
                            transport.id,
                            panic_text(payload)
                        ));
                        continue;
                    }
                };
                match catch_unwind(AssertUnwindSafe(|| {
                    run_case(&app, &fixture, &runtime.id, &transport.id)
                })) {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        failures.push(format!("{} x {}: {error:#}", runtime.id, transport.id))
                    }
                    Err(payload) => failures.push(format!(
                        "{} x {} panic: {}",
                        runtime.id,
                        transport.id,
                        panic_text(payload)
                    )),
                }
                fixtures.push(fixture);
            }
        }

        let final_workspace_ids = app.test_workspace_replica_ids();
        app.shutdown();
        drop(fixtures);
        assert_eq!(
            executed, expected_cases,
            "必须执行 runtime_list x transport_list 完整笛卡尔积"
        );
        assert!(
            failures.is_empty(),
            "GTK Runtime x Transport 完整矩阵失败（{executed} cases，pool={final_workspace_ids:?}）:\n{}",
            failures.join("\n\n")
        );
    });
}
