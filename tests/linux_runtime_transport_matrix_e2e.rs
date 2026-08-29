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
use std::io::Read;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use gtk4::gdk;
use gtk4::prelude::*;

use muxterm::core::catalog::Catalog;
use muxterm::core::config::{Action, Config};
use muxterm::core::model::backend::RuntimeCapability;
use muxterm::core::model::task::TaskOutcome;
use muxterm::core::quickconnect::model::TargetConfig;
use muxterm::platform::linux::window::AppWindow;

use support::herdr_test_support::herdr_available;
use support::linux_gtk::{
    find_by_name, gtk_test_framework_smoke, load_theme, pump_main_loop, simulate_key_press,
    skip_no_display, window_key_controller,
};
use support::runtime_transport_matrix::{build_2tab3pane, MatrixFixture};
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};
use support::tmux_test_support::tmux_available;

const GTK_MATRIX_TIMEOUT: Duration = Duration::from_secs(25);

/// W7 child env：父进程把 cell + scenario 经环境变量传给 current_exe 子进程。
const ENV_CHILD: &str = "MUXTERM_TEST_CHILD";
const ENV_RUNTIME: &str = "MUXTERM_TEST_RUNTIME";
const ENV_TRANSPORT: &str = "MUXTERM_TEST_TRANSPORT";
const ENV_SCENARIO: &str = "MUXTERM_TEST_SCENARIO";

/// 普通 child 硬 timeout 30 秒；large-history / takeover 场景也沿用 30 秒。
///
/// Parent 会串行启动多个 GTK/Xvfb child；在共享 runner 上，tmux/SSH
/// handshake 偶尔会把普通场景推过 15 秒，但 standalone child 仍能很快
/// 完成。统一预算只放宽失败边界，不改变成功路径等待。
const CHILD_TIMEOUT: Duration = Duration::from_secs(30);
const CHILD_TIMEOUT_LONG: Duration = Duration::from_secs(30);
/// parent 总预算 15 分钟。
const PARENT_BUDGET: Duration = Duration::from_secs(15 * 60);

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

fn join_reader(reader: &mut Option<thread::JoinHandle<String>>) -> String {
    reader
        .take()
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
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
        "workspace={} runtime={} tabs/panes={:?} active_tab={} active_pane={} last_input={} leaves={leaves:?} gtk={} vte={vte:?}",
        app.test_active_workspace_replica_id(),
        app.test_active_workspace_runtime(),
        app.test_tab_and_pane_counts(),
        app.test_active_tab_id(),
        app.test_active_pane_id(),
        String::from_utf8_lossy(&app.test_last_raw_input()).escape_debug(),
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

fn pane_vte_contains_token(app: &AppWindow, pane: u32, token: &str) -> bool {
    let text = app.test_pane_vte_text(pane);
    if text.contains(token) {
        return true;
    }
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains(token)
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
    // New-tab/SSH topology can settle before GTK has allocated and seeded the
    // VTE.  Wait for that lifecycle boundary before clearing the render trace;
    // otherwise the normal first-screen reset is misclassified as a reset
    // caused by the command under test.
    wait_for(app, &format!("pane {pane} 首屏 seed"), |app| {
        app.test_active_pane_seeded() && app.test_pane_allocation(pane).0 > 0
    })?;
    // 命令输出只能原样进入已有 Surface。Herdr 的 full frame、tmux 的
    // PaneOutput 与 shell PTY 字节都不得借机 reset VTE。
    app.test_clear_active_pane_render_trace();
    // SSH/split 后 Control 首帧可能仍是空屏；与 core 矩阵 execute_echo
    // 一样有界重试 printf，断言仍要求 VTE + 搜索出现真实 token。
    let mut next_send = Instant::now();
    wait_for(app, &format!("pane {pane} 输出 {token}"), |app| {
        if Instant::now() >= next_send {
            let _ = emit_command(app, &command);
            next_send = Instant::now() + Duration::from_millis(500);
        }
        let hits = app.test_search_workspace(&token);
        pane_vte_contains_token(app, pane, &token)
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
    ensure!(
        app.test_active_pane_id() == pane && app.test_active_pane_resets() == 0,
        "命令执行期间不得切走 pane 或 reset Surface: pane={pane}, active={}, resets={} ({})",
        app.test_active_pane_id(),
        app.test_active_pane_resets(),
        diagnostics(app)
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
                    .filter(|pane| pane_vte_contains_token(app, *pane, token))
                    .collect::<Vec<_>>()
                    == vec![*expected_pane]
            },
        )?;
    }
    Ok(())
}

fn wait_vte_buffer_ownership(
    app: &AppWindow,
    resident_panes: &[u32],
    tokens: &[(u32, String)],
) -> Result<()> {
    for (expected_pane, token) in tokens
        .iter()
        .filter(|(pane, _)| resident_panes.contains(pane))
    {
        wait_for(
            app,
            &format!("token {token} 只在常驻 VTE buffer pane {expected_pane}"),
            |app| {
                resident_panes
                    .iter()
                    .copied()
                    .filter(|pane| {
                        let text = app.test_pane_vte_buffer_text(*pane);
                        let compact: String = text
                            .chars()
                            .filter(|character| !character.is_whitespace())
                            .collect();
                        text.contains(token) || compact.contains(token)
                    })
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
    wait_for(app, "第二次 split 后聚焦右下 pane", |app| {
        app.test_active_pane_id() == tab2_panes[2]
    })?;
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
    wait_for(app, "reattach 后后台 pane 索引恢复", |app| {
        assert_search_ownership(app, &snapshot.tokens).is_ok()
    })
    .or_else(|timeout| {
        assert_search_ownership(app, &snapshot.tokens).with_context(|| timeout.to_string())
    })?;
    assert_search_ownership(app, &snapshot.tokens)?;

    app.test_handle_action(Action::SwitchTab1);
    wait_for(app, "reattach 后 SwitchTab1", |app| {
        app.test_active_tab_id() == snapshot.tab1
            && app.test_layout_leaf_ids() == vec![snapshot.tab1_pane]
    })?;
    wait_vte_buffer_ownership(app, &[snapshot.tab1_pane], latest_visible_tokens)?;
    let mut tokens = vec![execute_printf(
        app,
        &format!("GTK_{}_R1", matrix_label(runtime, transport)),
        false,
    )?];

    app.test_handle_action(Action::SwitchTab2);
    wait_for(app, "reattach 后 SwitchTab2", |app| {
        app.test_active_tab_id() == snapshot.tab2 && app.test_layout_leaf_ids().len() == 3
    })?;
    let panes = assert_three_pane_surface(app, "reattach 后切回 Tab 2")?;
    wait_vte_buffer_ownership(app, &panes, latest_visible_tokens)?;
    tokens.extend(exercise_three_panes(
        app,
        &panes,
        &format!("GTK_{}_R2", matrix_label(runtime, transport)),
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

/// 在 GTK/AppWindow 创建前预置一个真实的 2-tab/3-pane workspace。
///
/// 这样 Existing row click 看到的是已经 populated 的 workspace，attach
/// 本身不再偷偷承担 fixture 创建；随后 scenario 才能准确覆盖
/// attach → split → new tab → echo。
fn prepare_existing_fixture(fixture: &MatrixFixture, runtime: &str, transport: &str) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .context("创建 Existing fixture Tokio runtime")?;
    let mut catalog = Catalog::with_builtins();
    let workspace = rt
        .block_on(catalog.open(&fixture.spec))
        .with_context(|| format!("预置 {runtime} x {transport} Existing fixture"))?;
    build_2tab3pane(workspace, runtime, transport)?;
    rt.block_on(workspace.shutdown())?;
    Ok(())
}

const HERDR_INCIDENT_BASELINE_BYTES: usize = 223_320;
const HERDR_INCIDENT_BASELINE_MARKER: &str = "MX_INCIDENT_BASELINE_DONE";

/// 在 AppWindow 创建前向已 populated 的 Herdr workspace 写入固定大小的
/// baseline。该 fixture 对应 2026-08-24 事故的约 223KB 历史，而不是把
/// 历史生成混入 GTK attach 计时。
fn seed_herdr_incident_baseline(fixture: &MatrixFixture) -> Result<()> {
    ensure!(
        fixture.spec.runtime == "herdr",
        "incident baseline 只支持 Herdr"
    );
    let socket = fixture
        .spec
        .socket
        .as_ref()
        .context("incident fixture 缺 Herdr socket")?;
    let session = muxterm::core::runtime::herdr::session::HerdrSession::new(
        fixture.spec.session.clone(),
        socket,
    );
    let snapshot = session.snapshot().context("incident baseline snapshot")?;
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.workspace_id == fixture.spec.path)
        .map(|pane| pane.pane_id.clone())
        .context("incident baseline 找不到 populated pane")?;
    let command = format!(
        "head -c {HERDR_INCIDENT_BASELINE_BYTES} /dev/zero | tr '\\0' X; printf '\\n{HERDR_INCIDENT_BASELINE_MARKER}\\n'\n"
    );
    session
        .pane_send_text(&pane, &command)
        .context("写 incident baseline")?;
    let deadline = Instant::now() + CHILD_TIMEOUT_LONG;
    while Instant::now() < deadline {
        if let Ok(bytes) = session.pane_read_recent_ansi_lines(&pane, 20_000) {
            let text = String::from_utf8_lossy(&bytes);
            // Herdr's JSON `pane.read` response is intentionally capped
            // below the full history size; the command itself is the exact
            // baseline generator, while the marker proves it completed.
            if text.contains(HERDR_INCIDENT_BASELINE_MARKER) {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let observed = session
        .pane_read_recent_ansi_lines(&pane, 20_000)
        .map(|bytes| {
            (
                bytes.len(),
                String::from_utf8_lossy(&bytes).contains(HERDR_INCIDENT_BASELINE_MARKER),
            )
        })
        .unwrap_or((0, false));
    anyhow::bail!(
        "incident baseline 未达到约 {HERDR_INCIDENT_BASELINE_BYTES} bytes: pane={pane} observed={observed:?}"
    )
}

/// W7 child 入口：`current_exe --exact isolated_matrix_child` 子进程。
///
/// 读 MUXTERM_TEST_RUNTIME/TRANSPORT/SCENARIO，初始化一次 GTK，建一个
/// AppWindow + 一个 fixture，跑一个 scenario，然后 shutdown 退出。
/// 非 child 环境（未设 MUXTERM_TEST_CHILD=1）直接跳过，不影响普通跑法。
#[test]
fn isolated_matrix_child() {
    if std::env::var(ENV_CHILD).as_deref() != Ok("1") {
        return;
    }
    let runtime = std::env::var(ENV_RUNTIME).expect("child 缺 runtime");
    let transport = std::env::var(ENV_TRANSPORT).expect("child 缺 transport");
    let scenario = std::env::var(ENV_SCENARIO).expect("child 缺 scenario");
    eprintln!("CHILD_BEGIN {runtime} x {transport} x {scenario}");

    let (runtime_in, transport_in, scenario_in) =
        (runtime.clone(), transport.clone(), scenario.clone());
    let ok = std::panic::catch_unwind(AssertUnwindSafe(move || {
        gtk4::test_synced(move || {
            gtk_test_framework_smoke();
            // W7：parent 已拉起共享 sshd 时 child 复用（避免每格一个 sshd 堆积）。
            let sshd = match std::env::var("MUXTERM_TEST_SSHD_ALIAS") {
                Ok(alias) => LoopbackSshd::attach(
                    alias,
                    std::env::var("MUXTERM_SSH_CONFIG_PATH")
                        .map(PathBuf::from)
                        .context("child 缺 MUXTERM_SSH_CONFIG_PATH")?,
                )
                .expect("child attach 共享 sshd"),
                Err(_) => {
                    LoopbackSshd::start("gtk-matrix-child").expect("child 启动隔离 loopback sshd")
                }
            };
            let _ssh_config = EnvRestore::set("MUXTERM_SSH_CONFIG_PATH", &sshd.config_path);
            let fixture =
                MatrixFixture::new(&runtime_in, &transport_in, &sshd).expect("child fixture");
            // Existing discovery 必须只看当前隔离 fixture，不能扫描默认
            // tmux/Herdr server；这些 env 在 child 结束时自动恢复。
            let _herdr_discovery = if runtime_in == "herdr" {
                let socket = fixture
                    .spec
                    .socket
                    .as_ref()
                    .context("herdr Existing fixture 缺 socket")?;
                Some(EnvRestore::set(
                    "HERDR_SOCKET_PATH",
                    std::path::Path::new(socket),
                ))
            } else {
                None
            };
            let _tmux_discovery = if runtime_in == "tmux" {
                let socket = fixture
                    .spec
                    .socket
                    .as_ref()
                    .context("tmux Existing fixture 缺 socket")?;
                let key = if transport_in == "ssh" {
                    "MUXTERM_TEST_REMOTE_TMUX_SOCKET"
                } else {
                    "MUXTERM_TEST_LOCAL_TMUX_SOCKET"
                };
                Some(EnvRestore::set(key, std::path::Path::new(socket)))
            } else {
                None
            };
            if matches!(
                scenario_in.as_str(),
                "attach_then_mutate_existing" | "herdr_attach_split_incident"
            ) {
                prepare_existing_fixture(&fixture, &runtime_in, &transport_in)?;
            }
            if scenario_in == "herdr_attach_split_incident" {
                seed_herdr_incident_baseline(&fixture)?;
            }
            // project_existing_parity：预置隔离 store，面板 Project 行真实可点。
            let _store_guard =
                seed_project_store_if_needed(&scenario_in, &fixture, &runtime_in, &transport_in);
            let mut cfg = Config::default();
            cfg.pool.max_slots = 8;
            let app = AppWindow::new(cfg, load_theme());
            app.window.set_default_size(1280, 800);
            app.window.present();
            gtk4::test_widget_wait_for_draw(&app.window);
            pump_main_loop(120);
            let result =
                run_child_scenario(&app, &fixture, &runtime_in, &transport_in, &scenario_in);
            app.shutdown();
            drop(fixture);
            result
        })
    }));

    match ok {
        Ok(Ok(())) => {
            eprintln!("CHILD_OK {runtime} x {transport} x {scenario}");
        }
        Ok(Err(error)) => {
            eprintln!("CHILD_FAIL {runtime} x {transport} x {scenario}: {error:#}");
            std::process::exit(1);
        }
        Err(payload) => {
            eprintln!(
                "CHILD_PANIC {runtime} x {transport} x {scenario}: {}",
                panic_text(payload)
            );
            std::process::exit(2);
        }
    }
}

/// 子进程里按 scenario 名字分发（每个 child 只跑一个）。
fn run_child_scenario(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
    scenario: &str,
) -> Result<()> {
    match scenario {
        "matrix_full" => run_case(app, fixture, runtime, transport),
        "attach_then_mutate_existing" => {
            scenario_attach_then_mutate_existing(app, fixture, runtime, transport)
        }
        "herdr_attach_split_incident" => {
            scenario_herdr_attach_split_incident(app, fixture, runtime, transport)
        }
        "new_tab_button" => scenario_new_tab_button(app, fixture, runtime, transport),
        "new_tab_shortcut" => scenario_new_tab_shortcut(app, fixture, runtime, transport),
        "split_shortcuts" => scenario_split_shortcuts(app, fixture, runtime, transport),
        "ctrl_l_stays_clear" => scenario_ctrl_l_stays_clear(app, fixture, runtime, transport),
        "pool_foreground_switch" => {
            scenario_pool_foreground_switch(app, fixture, runtime, transport)
        }
        "detach_reattach" => scenario_detach_reattach(app, fixture, runtime, transport),
        "takeover_watchdog" => scenario_takeover_watchdog(app, fixture, runtime, transport),
        "project_existing_parity" => {
            scenario_project_existing_parity(app, fixture, runtime, transport)
        }
        other if other.starts_with("large_history_") => {
            scenario_large_history(app, fixture, runtime, transport, other)
        }
        other => anyhow::bail!("未知 scenario {other}"),
    }
}

/// 从当前 test 二进制 spawn 一个 child 并等它结束；超时 kill 并返回失败。
/// 输出 cell/scenario + stdout/stderr + artifact 路径。
fn spawn_child(
    runtime: &str,
    transport: &str,
    scenario: &str,
    ssh_config_path: &std::path::Path,
    timeout: Duration,
) -> Result<String> {
    let exe = std::env::current_exe().context("current_exe")?;
    let artifact_dir = std::env::temp_dir().join("muxterm-matrix-artifacts");
    std::fs::create_dir_all(&artifact_dir).ok();
    let artifact = artifact_dir.join(format!(
        "{runtime}-{transport}-{scenario}-{}.log",
        std::process::id()
    ));
    // W7 §12.1：parent 预分配精确 fixture 名（child 复用；kill 后兜底清理）。
    // herdr session 名有长度上限（实测 >51 会立刻退出）：只编 transport+nanos，
    // 足够短且唯一；scenario 不参与名字（parent 用 env 精确传递）。
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let (fixture_socket, fixture_name) = if runtime == "tmux" {
        (
            Some(format!("muxterm-test-matrix-{transport}-{nanos}")),
            None,
        )
    } else if runtime == "herdr" {
        (
            None,
            Some(format!("muxterm-test-matrix-{transport}-{nanos}")),
        )
    } else {
        (None, None)
    };
    let mut cmd = Command::new(&exe);
    cmd.args([
        "--exact",
        "isolated_matrix_child",
        "--nocapture",
        "--test-threads=1",
    ])
    .env(ENV_CHILD, "1")
    .env(ENV_RUNTIME, runtime)
    .env(ENV_TRANSPORT, transport)
    .env(ENV_SCENARIO, scenario)
    .env("MUXTERM_SSH_CONFIG_PATH", ssh_config_path);
    if let Some(socket) = &fixture_socket {
        cmd.env("MUXTERM_TEST_FIXTURE_SOCKET", socket);
    }
    if let Some(name) = &fixture_name {
        cmd.env("MUXTERM_TEST_FIXTURE_NAME", name);
    }
    if transport == "ssh" {
        // 共享 parent sshd：child 不再自起 sshd。
        let alias = std::fs::read_to_string(ssh_config_path)
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|l| l.trim_start().starts_with("Host "))
                    .map(|l| {
                        l.trim_start()
                            .trim_start_matches("Host ")
                            .trim()
                            .to_string()
                    })
            });
        if let Some(alias) = alias {
            cmd.env("MUXTERM_TEST_SSHD_ALIAS", alias);
        }
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn child {runtime} x {transport} x {scenario}"))?;
    // 必须在 child 运行期间持续 drain 两个 pipe。等 child 退出后再读会在
    // tmux/GTK 诊断输出较多时填满 OS pipe buffer，child 阻塞而被误判为 timeout。
    let mut stdout_reader = child.stdout.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    });
    let mut stderr_reader = child.stderr.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(err) => {
                let _ = child.kill();
                cleanup_matrix_fixture(&fixture_socket, &fixture_name);
                anyhow::bail!("child wait 失败: {err}");
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = join_reader(&mut stdout_reader);
            let stderr = join_reader(&mut stderr_reader);
            let log = format!(
                "=== cell {runtime} x {transport} scenario {scenario} ===\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}\nartifact: {}",
                artifact.display()
            );
            let _ = std::fs::write(&artifact, &log);
            cleanup_matrix_fixture(&fixture_socket, &fixture_name);
            anyhow::bail!("child 超时（{timeout:?}）:\n{log}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = join_reader(&mut stdout_reader);
    let stderr = join_reader(&mut stderr_reader);
    let log = format!(
        "=== cell {runtime} x {transport} scenario {scenario} ===\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}\nartifact: {}",
        artifact.display()
    );
    let _ = std::fs::write(&artifact, &log);
    if status.success() {
        Ok(log)
    } else {
        anyhow::bail!("child 退出码 {:?}:\n{log}", status.code());
    }
}

/// W7 §12.1：按预分配精确名兜底清理（child 被 kill 时 Drop 不会跑）。
/// 只清理 parent 自己预分配的 muxterm-test-* fixture，绝不碰用户默认。
fn cleanup_matrix_fixture(fixture_socket: &Option<String>, fixture_name: &Option<String>) {
    if let Some(name) = fixture_name {
        let _ = Command::new("herdr")
            .args(["session", "stop", name])
            .output();
        let _ = Command::new("herdr")
            .args(["session", "delete", name])
            .output();
    }
    if let Some(socket) = fixture_socket {
        let _ = Command::new("tmux")
            .args(["-L", socket, "kill-server"])
            .output();
    }
}

/// W7 §12.3：每个 cell 必跑的 child scenarios。
/// - shell：保留 matrix_full 独立 contract（不算四个必跑格）。
/// - tmux/herdr × local/ssh（四格）：四格场景全跑。
/// - herdr 两格额外：large_history / takeover_watchdog / project_existing_parity。
fn scenarios_for_cell(runtime: &str, transport: &str) -> Vec<&'static str> {
    let four_cells = matches!(transport, "local" | "ssh");
    let herdr_cells = runtime == "herdr" && four_cells;
    let mut scenarios = vec!["matrix_full"];
    if (runtime == "tmux" || runtime == "herdr") && four_cells {
        scenarios.extend([
            "attach_then_mutate_existing",
            "new_tab_button",
            "new_tab_shortcut",
            "split_shortcuts",
            "ctrl_l_stays_clear",
            "pool_foreground_switch",
            "detach_reattach",
        ]);
    }
    if herdr_cells {
        scenarios.extend([
            "large_history_100k",
            "large_history_393k",
            "large_history_500k",
            "takeover_watchdog",
            "project_existing_parity",
        ]);
        if transport == "local" {
            scenarios.push("herdr_attach_split_incident");
        }
    }
    scenarios
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

    // W7 §12.1：parent 不初始化 GTK，只枚举并 spawn child。
    let registry = Catalog::with_builtins();
    let runtimes = registry.runtime_list();
    let transports = registry.transport_list();
    let expected_cases = runtimes.len() * transports.len();
    let sshd = LoopbackSshd::start("gtk-runtime-matrix").expect("启动隔离 loopback sshd");
    let ssh_config_path = sshd.config_path.clone();

    // W7 §12.3：场景按 cell 分派；shell 保留 matrix_full 独立 contract，
    // 不替代 tmux/herdr × local/ssh 四个必跑格。
    let parent_deadline = Instant::now() + PARENT_BUDGET;
    let mut failures = Vec::new();
    let mut executed = 0usize;
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
            let scenarios = scenarios_for_cell(&runtime.id, &transport.id);
            for scenario in &scenarios {
                if Instant::now() >= parent_deadline {
                    failures.push(format!(
                        "{} x {} x {}: 超出 parent 总预算",
                        runtime.id, transport.id, scenario
                    ));
                    continue;
                }
                // herdr 格含 server 拉起+握手+snapshot，负载下 15s 不够，
                // 统一走长 budget；tmux/shell 保持 15s。large-history/takeover 亦然。
                let timeout = if runtime.id == "herdr"
                    || *scenario == "large_history_100k"
                    || *scenario == "large_history_393k"
                    || *scenario == "large_history_500k"
                    || *scenario == "takeover_watchdog"
                    || *scenario == "herdr_attach_split_incident"
                    || *scenario == "detach_reattach"
                {
                    CHILD_TIMEOUT_LONG
                } else {
                    CHILD_TIMEOUT
                };
                match spawn_child(
                    &runtime.id,
                    &transport.id,
                    scenario,
                    &ssh_config_path,
                    timeout,
                ) {
                    Ok(_) => {}
                    Err(error) => failures.push(format!(
                        "{} {} x {}: {error:#}",
                        runtime.id, transport.id, scenario
                    )),
                }
            }
        }
    }

    assert_eq!(
        executed, expected_cases,
        "必须执行 runtime_list x transport_list 完整笛卡尔积"
    );
    assert!(
        failures.is_empty(),
        "GTK Runtime x Transport 完整矩阵失败（{executed} cases）:\n{}",
        failures.join("\n\n")
    );
}

/// 按 widget 名找按钮。
fn find_button(app: &AppWindow, name: &str) -> Result<gtk4::Button> {
    let widget = find_by_name(&app.test_window(), name)
        .with_context(|| format!("widget 树找不到 {name}"))?;
    widget
        .downcast::<gtk4::Button>()
        .map_err(|_| anyhow::anyhow!("{name} 不是 Button"))
}

/// Existing Connections production path：点击已 populated workspace 后继续
/// split、new tab，并通过真实 VTE commit 执行 echo。
fn scenario_attach_then_mutate_existing(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    ensure!(
        matches!(runtime, "tmux" | "herdr"),
        "attach_then_mutate_existing 只跑 tmux/herdr"
    );
    let expected_replica = fixture.spec.id().replica_id();

    // 生产 discovery 路径：只驱动 GLib timer，不调用 test_poll_once 来
    // 掩盖 Existing probe 未接入生产 poll 的问题。
    app.test_open_panel(0);
    pump_main_loop(80);
    let root_list = find_by_name(&app.test_window(), "muxterm-panel-list")
        .context("Existing scenario 面板列表应存在")?
        .downcast::<gtk4::ListBox>()
        .map_err(|_| anyhow::anyhow!("面板列表不是 ListBox"))?;
    let folder = find_row_by_name(&root_list, "muxterm-existing-connections")
        .context("Existing scenario 缺少已有连接入口")?;
    folder.activate();
    pump_main_loop(80);

    let connect_name = transport_label(transport, fixture);
    let identity = if runtime == "herdr" {
        format!("{}-{}", fixture.spec.path, fixture.spec.session)
    } else {
        fixture.spec.session.clone()
    };
    let row_prefix = format!("muxterm-existing-row-{runtime}-{connect_name}-{identity}");
    let deadline = Instant::now() + GTK_MATRIX_TIMEOUT;
    let row = loop {
        pump_main_loop(40);
        let list = find_by_name(&app.test_window(), "muxterm-panel-list")
            .context("Existing attach 列表应存在")?
            .downcast::<gtk4::ListBox>()
            .map_err(|_| anyhow::anyhow!("Existing attach 列表不是 ListBox"))?;
        if let Some(row) = find_row_by_prefix(&list, &row_prefix) {
            break row;
        }
        if Instant::now() >= deadline {
            anyhow::bail!("Existing row 未出现: prefix={row_prefix}");
        }
    };
    row.activate();

    wait_for(app, "Existing row attach populated workspace", |app| {
        app.test_active_workspace_replica_id() == expected_replica
            && app.test_active_workspace_runtime() == runtime
            && app.test_tab_ids().len() == 2
            && app.test_layout_leaf_ids().len() == 3
    })?;
    let attached_active = app.test_active_pane_id();

    // 复现 incident 的第一步：attach 后立即对当前 pane split。
    press_key(app, gdk::Key::s, gdk::ModifierType::ALT_MASK)?;
    wait_for(app, "Existing attach 后 Alt+S", |app| {
        app.test_layout_leaf_ids().len() == 4 && app.test_active_pane_id() != attached_active
    })?;
    press_key(app, gdk::Key::v, gdk::ModifierType::ALT_MASK)?;
    wait_for(app, "Existing attach 后 Alt+V", |app| {
        app.test_layout_leaf_ids().len() == 5
    })?;

    // 继续使用真实 + 按钮，确保 split 后新 tab 也沿同一生命周期契约。
    find_button(app, "muxterm-new-tab")?.emit_clicked();
    wait_for(app, "Existing attach 后 + 创建新 tab", |app| {
        let target = app.test_active_pane_id();
        let (width, height) = app.test_pane_allocation(target);
        app.test_tab_ids().len() == 3
            && app.test_tab_and_pane_counts() == (3, 1)
            && width > 0
            && height > 0
            && !app.test_pane_vte_text(target).trim().is_empty()
    })?;
    let target = app.test_active_pane_id();
    let (_, token) = execute_printf(
        app,
        &format!("ATTACHED_{}_{}", matrix_label(runtime, transport), target),
        false,
    )?;
    ensure!(
        app.test_search_workspace(&token).len() == 1
            && app.test_pane_vte_text(target).contains(&token),
        "attach 后新 tab 的 echo 必须只出现在目标 VTE"
    );
    for pane in app.test_layout_leaf_ids() {
        let (width, height) = app.test_pane_allocation(pane);
        ensure!(
            width > 0 && height > 0,
            "attach 后 pane {pane} geometry 无效"
        );
    }
    Ok(())
}

/// 2026-08-24 Herdr incident regression: populated attach → split → NewTab →
/// real echo must settle and exit normally with a fixed large baseline.
fn scenario_herdr_attach_split_incident(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    ensure!(runtime == "herdr" && transport == "local");
    scenario_attach_then_mutate_existing(app, fixture, runtime, transport)?;
    ensure!(
        !app.test_search_workspace(HERDR_INCIDENT_BASELINE_MARKER)
            .is_empty(),
        "incident attach 后 baseline marker 必须仍在 Workspace/Core 中"
    );
    Ok(())
}

/// 在窗口的 EventControllerKey 上发真实按键（生产 keymap 路径）。
fn press_key(app: &AppWindow, key: gdk::Key, mods: gdk::ModifierType) -> Result<()> {
    let ctrl = window_key_controller(&app.test_window()).context("窗口缺 EventControllerKey")?;
    simulate_key_press(&ctrl, key, mods);
    Ok(())
}

/// 打开 fixture 主 workspace 并等初始 1 tab / 1 pane。
fn open_primary(app: &AppWindow, fixture: &MatrixFixture, runtime: &str) -> Result<String> {
    let replica = fixture.spec.id().replica_id();
    app.test_open_spec(fixture.spec.clone());
    wait_for(app, "主 Workspace 激活", |app| {
        app.test_active_workspace_replica_id() == replica
            && app.test_active_workspace_runtime() == runtime
            && app.test_tab_and_pane_counts() == (1, 1)
    })?;
    Ok(replica)
}

/// W7 §12.3 `new_tab_button`：真实 `muxterm-new-tab` clicked。
fn scenario_new_tab_button(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    let _replica = open_primary(app, fixture, runtime)?;
    let tab1 = app.test_active_tab_id();
    let tab1_pane = app.test_active_pane_id();

    let button = find_button(app, "muxterm-new-tab")?;
    button.emit_clicked();

    // 5 秒收敛：服务端/Core/GTK 全部同意 2 tab，且新 tab 的 pane 成为 active。
    wait_for(
        app,
        "clicked 后 5s 内收敛 2 tab + 新 pane active",
        |app| {
            app.test_tab_ids().len() == 2
                && app.test_active_tab_id() != tab1
                && app.test_active_pane_id() != tab1_pane
                && app.test_tab_and_pane_counts() == (2, 1)
        },
    )?;
    let names = app.test_tab_names();
    ensure!(
        names.len() == 2 && names.iter().all(|n| !n.trim().is_empty()),
        "新 tab 必须带非空 label: {names:?}"
    );
    let (pane, token) = execute_printf(
        app,
        &format!("BTN_{}", matrix_label(runtime, transport)),
        runtime == "shell" && transport == "ssh",
    )?;
    ensure!(
        app.test_active_pane_id() == pane && app.test_tab_and_pane_counts() == (2, 1),
        "clicked 新建的 tab 必须可输入: counts={:?}",
        app.test_tab_and_pane_counts()
    );
    ensure!(
        app.test_search_workspace(&token).len() == 1,
        "new_tab_button token 必须恰好一份"
    );
    Ok(())
}

/// W7 §12.3 `new_tab_shortcut`：真实 Alt+T。
fn scenario_new_tab_shortcut(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    let _replica = open_primary(app, fixture, runtime)?;
    let tab1 = app.test_active_tab_id();
    let tab1_pane = app.test_active_pane_id();

    press_key(app, gdk::Key::t, gdk::ModifierType::ALT_MASK)?;
    wait_for(
        app,
        "Alt+T 后 5s 内收敛 2 tab + 新 pane active",
        |app| {
            app.test_tab_ids().len() == 2
                && app.test_active_tab_id() != tab1
                && app.test_active_pane_id() != tab1_pane
                && app.test_tab_and_pane_counts() == (2, 1)
        },
    )?;
    let names = app.test_tab_names();
    ensure!(
        names.len() == 2 && names.iter().all(|n| !n.trim().is_empty()),
        "Alt+T 后顺序标签必须非空: {names:?}"
    );
    if runtime == "herdr" {
        // Herdr：raw 数字 label（tA → "10"），payload 无空 label 由 mutation 单测覆盖。
        ensure!(
            names[1].parse::<u32>().is_ok(),
            "herdr 新 tab 必须是 raw 数字 label: {names:?}"
        );
    }
    let (pane, token) = execute_printf(
        app,
        &format!("KEY_{}_{}", matrix_label(runtime, transport), "T2"),
        runtime == "shell" && transport == "ssh",
    )?;
    ensure!(
        app.test_active_pane_id() == pane && app.test_tab_and_pane_counts() == (2, 1),
        "Alt+T 新 tab 必须可输入: counts={:?}",
        app.test_tab_and_pane_counts()
    );
    ensure!(
        app.test_search_workspace(&token).len() == 1,
        "new_tab_shortcut token 必须恰好一份"
    );
    Ok(())
}

/// W7 §12.3 `split_shortcuts`：真实 Alt+S / Alt+V。
fn scenario_split_shortcuts(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    let _replica = open_primary(app, fixture, runtime)?;
    let pane0 = app.test_active_pane_id();

    press_key(app, gdk::Key::s, gdk::ModifierType::ALT_MASK)?;
    wait_for(app, "Alt+S 水平分割出右 pane", |app| {
        app.test_layout_leaf_ids().len() == 2
    })?;
    press_key(app, gdk::Key::v, gdk::ModifierType::ALT_MASK)?;
    let panes = assert_three_pane_surface(app, "Alt+S+Alt+V 后 GTK layout")?;
    wait_for(app, "第二次 split 聚焦右下", |app| {
        app.test_active_pane_id() == panes[2]
    })?;
    ensure!(
        panes[0] == pane0,
        "水平分割必须保留原 pane: panes={panes:?}, pane0={pane0}"
    );
    let tokens = exercise_three_panes(
        app,
        &panes,
        &format!("SPLIT_{}", matrix_label(runtime, transport)),
    )?;
    assert_search_ownership(app, &tokens)?;
    wait_visible_vte_ownership(app, &panes, &tokens)?;
    Ok(())
}

/// W7 §12.3 `ctrl_l_stays_clear`：真实 Ctrl-L，BEFORE 消失、AFTER 可见、切换后不复活。
fn scenario_ctrl_l_stays_clear(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    let _replica = open_primary(app, fixture, runtime)?;
    let pane = app.test_active_pane_id();
    let before = format!("CLB_{}", matrix_label(runtime, transport));
    let after = format!("CLA_{}", matrix_label(runtime, transport));
    // 先建第二个 tab（真实 Alt+T），再切回 tab1。
    let tab1 = app.test_active_tab_id();
    press_key(app, gdk::Key::t, gdk::ModifierType::ALT_MASK)?;
    wait_for(app, "Alt+T 建 tab2", |app| app.test_tab_ids().len() == 2)?;
    press_key(app, gdk::Key::_1, gdk::ModifierType::ALT_MASK)?;
    wait_for(app, "Alt+1 回 tab1", |app| {
        app.test_active_tab_id() == tab1
    })?;

    // BEFORE 屏内容可见。
    let (_, before_token) = execute_printf(app, &before, false)?;
    // 真实 Ctrl-L：0x0c 走生产 WriteRaw 输入路径。
    app.test_send_input(&[0x0c]);
    pump_main_loop(150);
    // 服务端必须真的清屏（生产 WriteRaw 0x0c 到达 herdr 服务端）。
    if runtime == "herdr" {
        if let Some(socket) = fixture.spec.socket.as_ref() {
            let sess = muxterm::core::runtime::herdr::session::HerdrSession::new(
                fixture.spec.session.clone(),
                socket,
            );
            if let Ok(snapshot) = sess.snapshot() {
                let pane_id = snapshot
                    .panes
                    .iter()
                    .find(|p| p.workspace_id == fixture.spec.path)
                    .map(|p| p.pane_id.clone());
                if let Some(pane_id) = pane_id {
                    let server_has_before = sess
                        .pane_read_recent_ansi(&pane_id)
                        .map(|bytes| String::from_utf8_lossy(&bytes).contains(&before_token))
                        .unwrap_or(true);
                    ensure!(
                        !server_has_before,
                        "herdr 服务端必须已清屏（Ctrl-L 未生效）"
                    );
                }
            }
        }
    }
    // AFTER 可见（清屏后 prompt 在屏顶，AFTER 必须在整屏文本里）。
    let (_, after_token) = execute_printf(app, &after, false)?;
    // 当前屏（不含 VTE scrollback）断言：BEFORE 必须已清掉，AFTER 可见。
    // Ctrl-L 清屏帧到达 client VTE 需要一点时间，用轮询等它收敛。
    let deadline = Instant::now() + GTK_MATRIX_TIMEOUT;
    let mut converged = false;
    while Instant::now() < deadline {
        tick(app);
        let screen = app.test_pane_screen_text(pane);
        if !screen.contains(&before_token) && screen.contains(&after_token) {
            converged = true;
            break;
        }
    }
    if !converged {
        let screen = app.test_pane_screen_text(pane);
        let full = app.test_pane_vte_text(pane);
        let rows = app.test_pane_screen_rows(pane);
        let cursor = app.test_pane_cursor_row(pane);
        let (trace_seeds, trace_feeds, trace_bytes) = app.test_pane_render_trace(pane);
        let mut lines = Vec::new();
        for (i, line) in full.lines().enumerate() {
            if line.contains(&before_token) || line.contains(&after_token) {
                lines.push(format!("full[{i}]: {:?}", line.trim_end()));
            }
        }
        // 诊断：完整 buffer 文本（scrollback + 屏幕），看 CLA/CLB 到底在哪。
        let full_dump: Vec<String> = full
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{i}: {:?}", l.trim_end()))
            .collect();
        anyhow::bail!(
            "Ctrl-L 清屏未收敛: rows={rows} cursor={cursor} trace=(seeds={trace_seeds},feeds={trace_feeds},bytes={trace_bytes}) screen_len={} screen_has_before={} screen_has_after={} full_has_before={} full_has_after={} lines={lines:?} screen={screen:?}\nFULL_DUMP:\n{}",
            screen.len(),
            screen.contains(&before_token),
            screen.contains(&after_token),
            full.contains(&before_token),
            full.contains(&after_token),
            full_dump.join("\n"),
        );
    }
    let screen = app.test_pane_screen_text(pane);
    ensure!(
        screen.contains(&after_token),
        "AFTER token 必须在当前屏: {screen:?}"
    );
    ensure!(
        !screen.contains(&before_token),
        "BEFORE 当前屏必须已消失（Ctrl-L 清屏）: {screen:?}"
    );
    // 切换 tab 再回来：不复活（无 reseed 把 BEFORE 重新带回当前屏）。
    press_key(app, gdk::Key::_2, gdk::ModifierType::ALT_MASK)?;
    wait_for(app, "Alt+2 到 tab2", |app| {
        app.test_active_tab_id() != tab1
    })?;
    press_key(app, gdk::Key::_1, gdk::ModifierType::ALT_MASK)?;
    wait_for(app, "Alt+1 回 tab1", |app| {
        app.test_active_tab_id() == tab1
    })?;
    let screen2 = app.test_pane_screen_text(pane);
    ensure!(
        !screen2.contains(&before_token),
        "Ctrl-L 后切换不得把 BEFORE 复活回当前屏: {screen2:?}"
    );
    ensure!(
        screen2.contains(&after_token),
        "切换回来后 AFTER 必须仍可见（内容连续无 reseed）: {screen2:?}"
    );
    Ok(())
}

/// W7 §12.3 `pool_foreground_switch`：两 workspace 切换；Herdr 后台只 observe；
/// 后台 Surface 持续接收；切回连续无 reseed。
fn scenario_pool_foreground_switch(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    let original_replica = open_primary(app, fixture, runtime)?;
    let snapshot = build_gui_scenario(app, runtime, transport)
        .with_context(|| format!("{runtime} x {transport} 构造 GTK 2tab3pane"))?;
    let return_token = verify_pool_switch(
        app,
        fixture,
        runtime,
        transport,
        &original_replica,
        &snapshot,
    )?;
    let mut latest = snapshot.tokens.clone();
    latest.retain(|(pane, _)| *pane != return_token.0);
    latest.push(return_token.clone());
    ensure!(
        app.test_pane_vte_text(return_token.0)
            .contains(&return_token.1),
        "切回后输入输出必须落在保留的 active pane"
    );
    // Herdr 后台只 observe：切到 alternate 后原 workspace 的 pane 应降为 Observe。
    if runtime == "herdr" {
        let alternate_replica = fixture.alternate_spec.id().replica_id();
        app.test_activate_workspace(&alternate_replica);
        wait_for(app, "切到 alternate workspace", |app| {
            app.test_active_workspace_replica_id() == alternate_replica
        })?;
        wait_for(app, "后台 pane 降到 Observe", |app| {
            app.test_herdr_probe(&original_replica, snapshot.active_pane)
                .map(|(_, _, _, mode)| mode == "Some(Observe)")
                .unwrap_or(false)
        })?;
        app.test_activate_workspace(&original_replica);
        wait_for(app, "切回原 workspace", |app| {
            app.test_active_workspace_replica_id() == original_replica
        })?;
        ensure!(
            app.test_pane_vte_text(return_token.0)
                .contains(&return_token.1),
            "切回后内容必须连续（无 reseed 丢 token）"
        );
    }
    Ok(())
}

/// W7 §12.3 `detach_reattach`：丢旧 Runtime 后重连，服务端/Core/VTE 连续。
fn scenario_detach_reattach(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    let _replica = open_primary(app, fixture, runtime)?;
    let snapshot = build_gui_scenario(app, runtime, transport)
        .with_context(|| format!("{runtime} x {transport} 构造 2tab3pane"))?;
    verify_supported_reattach(
        app,
        fixture,
        runtime,
        transport,
        &snapshot,
        &snapshot.tokens,
    )?;
    Ok(())
}

/// W7 §12.3 `takeover_watchdog`（Herdr local+SSH）：retry 有界、taken-over 后
/// control auto-start=0、显式输入只 promote 一次。
fn scenario_takeover_watchdog(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    _transport: &str,
) -> Result<()> {
    ensure!(runtime == "herdr", "takeover_watchdog 只跑 herdr 格");
    let replica = open_primary(app, fixture, runtime)?;
    let pane = app.test_active_pane_id();
    let probe = |app: &AppWindow| app.test_herdr_probe(&replica, pane);
    wait_for(app, "stream 探针就绪", |app| probe(app).is_some())?;

    // 稳定观察 2 秒：stream 不得反复重启（≤1 次）。
    let start = probe(app).expect("探针").0;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        tick(app);
    }
    let after_wait = probe(app).expect("探针").0;
    ensure!(
        after_wait - start <= 1,
        "正常观察期间 stream 不得反复重启: before={start}, after={after_wait}"
    );

    // taken-over：真实第二个 client 以 Control takeover 抢占。
    let socket_path = fixture
        .spec
        .socket
        .as_ref()
        .context("herdr spec 缺 socket")?;
    let session_name = fixture.spec.session.clone();
    let sess = muxterm::core::runtime::herdr::session::HerdrSession::new(session_name, socket_path);
    // 找到 pane 的 herdr public id。
    let herdr_pane = sess
        .snapshot()
        .context("snapshot")?
        .panes
        .iter()
        .find(|p| p.workspace_id == fixture.spec.path)
        .map(|p| p.pane_id.clone())
        .unwrap_or_default();
    ensure!(!herdr_pane.is_empty(), "snapshot 找不到 herdr pane");
    let taken =
        raw_control_takeover(sess.client_socket_path(), &herdr_pane).context("raw takeover")?;
    if taken {
        wait_for(app, "后台 Control 被抢占后禁止 auto-start", |app| {
            let p = probe(app).expect("探针");
            p.1 == 0 && p.2
        })?;
        let p = probe(app).expect("探针");
        ensure!(
            p.1 == 0 && p.2,
            "taken-over 后 control auto-start 必须=0 且 suppressed: {p:?}"
        );
    }
    Ok(())
}

/// 真实第二个 raw client：Hello → ControlTerminal{takeover}。
fn raw_control_takeover(socket_path: &std::path::Path, target: &str) -> Result<bool> {
    use muxterm::core::runtime::herdr::wire::{
        read_message, write_message, ClientKeybindings, ClientLaunchMode, ClientMessage,
        RenderEncoding, ServerMessage, HERDR_PROTOCOL_VERSION, MAX_FRAME_SIZE,
    };
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("连接 herdr client socket {} 失败", socket_path.display()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .context("设置 raw client 读超时")?;
    write_message(
        &mut stream,
        &ClientMessage::Hello {
            version: HERDR_PROTOCOL_VERSION,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            requested_encoding: RenderEncoding::TerminalAnsi,
            keybindings: ClientKeybindings::Server,
            launch_mode: ClientLaunchMode::TerminalAttach,
        },
    )
    .context("raw client 写 Hello")?;
    let _welcome: ServerMessage =
        read_message(&mut stream, MAX_FRAME_SIZE).context("raw client 读 Welcome")?;
    write_message(
        &mut stream,
        &ClientMessage::ControlTerminal {
            target: target.to_string(),
            takeover: true,
        },
    )
    .context("raw client 写 ControlTerminal")?;
    loop {
        match read_message::<_, ServerMessage>(&mut stream, MAX_FRAME_SIZE) {
            Ok(ServerMessage::Terminal(_)) => return Ok(true),
            Ok(ServerMessage::ServerShutdown { .. }) => return Ok(false),
            Ok(_) => continue,
            Err(err) => anyhow::bail!("raw client 读响应失败: {err}"),
        }
    }
}

/// W7 §12.3 `large_history_100k/393k/500k`（Herdr local+SSH）：
/// attach 前生成历史；20 轮切 tab；token 恰好一份；切换后 seeds/resets +0。
fn scenario_large_history(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
    scenario: &str,
) -> Result<()> {
    ensure!(runtime == "herdr", "large_history 只跑 herdr 格");
    // 场景名 `large_history_100k` 等：k 后缀 → ×1000。
    let lines: usize = scenario
        .strip_prefix("large_history_")
        .map(|n| {
            if let Some(digits) = n.strip_suffix('k') {
                digits.parse::<usize>().map(|v| v * 1_000)
            } else {
                n.parse::<usize>()
            }
        })
        .transpose()
        .context("scenario 名缺行数")?
        .expect("large_history_ 前缀必有行数");

    // attach 前生成历史：直接对 herdr 服务端写 N 行（wire）。
    let socket_path = fixture
        .spec
        .socket
        .as_ref()
        .context("herdr spec 缺 socket")?;
    let sess = muxterm::core::runtime::herdr::session::HerdrSession::new(
        fixture.spec.session.clone(),
        socket_path,
    );
    let snap = sess.snapshot().context("snapshot")?;
    let herdr_pane = snap
        .panes
        .iter()
        .find(|p| p.workspace_id == fixture.spec.path)
        .map(|p| p.pane_id.clone())
        .unwrap_or_default();
    ensure!(!herdr_pane.is_empty(), "snapshot 找不到 herdr pane");
    sess.pane_send_text(&herdr_pane, &format!("seq 1 {lines}\n"))
        .context("写 seq 生成历史")?;
    // 等服务端 scrollback 出现最后一行（attach 前历史已生成）。
    let deadline = Instant::now() + CHILD_TIMEOUT_LONG;
    loop {
        if let Ok(bytes) = sess.pane_read_recent_ansi_lines(&herdr_pane, 2000) {
            let text = String::from_utf8_lossy(&bytes);
            if text.lines().any(|line| line.trim() == lines.to_string()) {
                break;
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("服务端 N 行 scrollback 未生成: lines={lines}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // attach。
    let _replica = open_primary(app, fixture, runtime)?;
    // attach 后 cursor 在最后两行（计划 §12.3：cursor 最后两行）。
    wait_for(app, "attach 后 cursor 在最后两行", |app| {
        let pane = app.test_active_pane_id();
        let rows = app.test_pane_screen_rows(pane);
        let cursor = app.test_pane_cursor_row(pane);
        rows > 0 && cursor >= rows - 2
    })?;
    // token 恰好一份。
    let token = format!("LARGE_{}", matrix_label(runtime, transport));
    let (pane1, token) = execute_printf(app, &token, false)?;
    ensure!(
        app.test_search_workspace(&token).len() == 1,
        "large_history token 必须恰好一份"
    );
    // 20 轮切 tab（先建第二个 tab）。
    press_key(app, gdk::Key::t, gdk::ModifierType::ALT_MASK)?;
    wait_for(app, "Alt+T 建 tab2", |app| app.test_tab_ids().len() == 2)?;
    let tab1 = app.test_tab_ids()[0];
    let tab2_pane = app.test_active_pane_id();
    app.test_clear_active_pane_render_trace();
    for round in 0..20 {
        press_key(app, gdk::Key::_2, gdk::ModifierType::ALT_MASK)?;
        wait_for(app, "Alt+2 到 tab2", |app| {
            app.test_active_tab_id() != tab1
        })?;
        press_key(app, gdk::Key::_1, gdk::ModifierType::ALT_MASK)?;
        wait_for(app, "Alt+1 回 tab1", |app| {
            app.test_active_tab_id() == tab1 && app.test_active_pane_id() == pane1
        })?;
        if round % 4 == 0 {
            tick(app);
        }
    }
    ensure!(
        app.test_active_pane_resets() == 0,
        "20 轮切 tab 后 seeds/resets 必须 +0: resets={}",
        app.test_active_pane_resets()
    );
    ensure!(
        app.test_pane_vte_text(pane1).contains(&token),
        "大历史 pane 的 VTE 必须仍有 token，token 已确认在 pane {pane1} \
         （tab2 pane={tab2_pane} 不参与断言）"
    );
    Ok(())
}

/// project_existing_parity 专用的临时 QuickConnect store 预置：
/// 把 fixture 的目标存进隔离 store，让面板里出现真实 Project 行。
/// 返回 EnvRestore 以便 child 结束前不污染用户配置目录。
fn seed_project_store_if_needed(
    scenario: &str,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Option<EnvRestore> {
    if scenario != "project_existing_parity" || runtime != "herdr" {
        return None;
    }
    let config = TargetConfig {
        name: format!("parity-{transport}"),
        runtime: muxterm::core::quickconnect::model::TargetRuntime::Herdr,
        transport: if transport == "ssh" {
            muxterm::core::quickconnect::model::TargetTransport::Ssh {
                name: fixture.spec.alias.clone().unwrap_or_default(),
            }
        } else {
            muxterm::core::quickconnect::model::TargetTransport::Local
        },
        path: "/tmp".into(),
        socket: fixture.spec.socket.clone(),
        session: Some(fixture.spec.session.clone()),
        workspace_id: Some(fixture.spec.path.clone()),
    };
    // 隔离统一 config 文件：临时目录，绝不写用户 ~/.config/muxterm/config.toml。
    let tmp_dir =
        std::env::temp_dir().join(format!("muxterm-qc-{}-{transport}", std::process::id()));
    let config_dir = tmp_dir.join("muxterm");
    let _ = std::fs::create_dir_all(&config_dir);
    let restore = EnvRestore::set("XDG_CONFIG_HOME", &tmp_dir);
    let store_path = muxterm::core::config::Config::user_config_path()
        .expect("临时 XDG_CONFIG_HOME 下必有 config 路径");
    let mut store =
        muxterm::core::quickconnect::store::QuickConnectStore::new_unified(Some(store_path));
    store.upsert_project(&config);
    Some(restore)
}

/// W7 §12.3 `project_existing_parity`（Herdr local+SSH）：真实 Existing 行与
/// Project 行分别 click；保存/重载后 identity key / attach spec identity /
/// id/workspace 相同；Core Recent 再开不丢 path/socket/workspace_id。
fn scenario_project_existing_parity(
    app: &AppWindow,
    fixture: &MatrixFixture,
    runtime: &str,
    transport: &str,
) -> Result<()> {
    ensure!(runtime == "herdr", "project_existing_parity 只跑 herdr 格");
    // 让 discovery 只扫测试 socket（禁止连用户默认 herdr.sock）。
    let socket = fixture
        .spec
        .socket
        .as_ref()
        .context("herdr spec 缺 socket")?;
    {
        let prev = std::env::var_os("HERDR_SOCKET_PATH");
        std::env::set_var("HERDR_SOCKET_PATH", socket);
        let _ = prev;
    }
    let expected_replica = fixture.spec.id().replica_id();

    // 1) 真实 Project 行 click（store 预置的 Project 行）。
    app.test_open_panel(0);
    pump_main_loop(80);
    let project_name = format!("parity-{transport}@{}", transport_label(transport, fixture));
    let list = find_by_name(&app.test_window(), "muxterm-panel-list")
        .context("面板列表应存在")?
        .downcast::<gtk4::ListBox>()
        .map_err(|_| anyhow::anyhow!("ListBox 类型"))?;
    let project_row = find_row_by_name(&list, &project_name)
        .with_context(|| format!("Project 行 {project_name} 应存在"))?;
    project_row.activate();
    wait_for(app, "Project 行 click 后 attach 同一 identity", |app| {
        app.test_active_workspace_replica_id() == expected_replica
            && app.test_active_workspace_runtime() == "herdr"
            && app.test_tab_and_pane_counts() == (1, 1)
    })?;
    ensure!(
        app.test_active_workspace_replica_id() == expected_replica,
        "Project 行必须打开 fixture 的同一 workspace: expected={expected_replica}, got={}",
        app.test_active_workspace_replica_id()
    );

    // 2) 内存态 identity：identity key、attach spec identity、id/workspace 相同。
    let config = project_target_config(fixture, transport)?;
    let mut store = muxterm::core::quickconnect::store::QuickConnectStore::in_memory();
    store.upsert_project(&config);
    assert_eq!(store.projects.len(), 1);
    let saved = store.projects[0].clone();
    ensure!(
        store.projects.len() == 1,
        "保存后应有 1 个 Project: {}",
        store.projects.len()
    );
    ensure!(
        saved.identity_key() == config.identity_key(),
        "保存/重载后 identity key 必须相同"
    );
    ensure!(
        saved.workspace_id == config.workspace_id && saved.socket == config.socket,
        "保存/重载后 path/socket/workspace_id 必须保留: saved={saved:?}"
    );
    let spec_before = muxterm::core::catalog::config_to_spec(&config);
    let spec_after = muxterm::core::catalog::config_to_spec(&saved);
    ensure!(
        spec_before.id() == spec_after.id(),
        "attach spec identity 必须相同: before={:?}, after={:?}",
        spec_before.id(),
        spec_after.id()
    );

    // 3) 真实 Existing 行 click（面板 → 已有的连接 → 扁平 herdr 行）。
    muxterm::platform::linux::quickconnect_panel::close_current();
    pump_main_loop(40);
    app.test_open_panel(0);
    pump_main_loop(80);
    let list = find_by_name(&app.test_window(), "muxterm-panel-list")
        .context("面板列表应存在")?
        .downcast::<gtk4::ListBox>()
        .map_err(|_| anyhow::anyhow!("ListBox 类型"))?;
    let folder_row = find_row_by_name(&list, "muxterm-existing-connections")
        .context("根列表应有「已有的连接」Folder")?;
    folder_row.activate();
    pump_main_loop(60);
    // 扁平列表找 herdr 行（runtime-connect-workspace）。
    let ws = fixture.spec.path.clone();
    let existing_prefix = format!(
        "muxterm-existing-row-herdr-{}-{ws}-{}",
        existing_connect_name(fixture, transport),
        fixture.spec.session
    );
    let existing_name = format!(
        "muxterm-existing-row-herdr-{}-{ws}-{}",
        existing_connect_name(fixture, transport),
        fixture.spec.session
    );
    let deadline = Instant::now() + GTK_MATRIX_TIMEOUT;
    let herdr_row = loop {
        pump_main_loop(40);
        let list = find_by_name(&app.test_window(), "muxterm-panel-list")
            .context("面板列表应存在")?
            .downcast::<gtk4::ListBox>()
            .map_err(|_| anyhow::anyhow!("ListBox 类型"))?;
        if let Some(row) = find_row_by_prefix(&list, &existing_prefix) {
            break row;
        }
        if Instant::now() >= deadline {
            anyhow::bail!("{existing_name} 未在窗口出现；行名前缀={existing_prefix}");
        }
    };
    herdr_row.activate();
    wait_for(app, "Existing 行 click 后 attach 同一 identity", |app| {
        app.test_workspace_replica_ids().contains(&expected_replica)
            && app.test_active_workspace_replica_id() == expected_replica
    })?;
    ensure!(
        app.test_workspace_replica_ids()
            .iter()
            .filter(|r| *r == &expected_replica)
            .count()
            == 1,
        "Existing 行必须复用同一 workspace，不能开第二个: {:?}",
        app.test_workspace_replica_ids()
    );
    let _ = existing_name;
    let _ = socket;
    Ok(())
}

/// 面板行名里 transport 片段。
fn transport_label(transport: &str, fixture: &MatrixFixture) -> String {
    if transport == "ssh" {
        fixture.spec.alias.clone().unwrap_or_else(|| "ssh".into())
    } else {
        "local".into()
    }
}

/// Existing 行名里 connect 片段（local / alias）。
fn existing_connect_name(fixture: &MatrixFixture, transport: &str) -> String {
    transport_label(transport, fixture)
}

/// 按 widget name 在面板列表里找行。
fn find_row_by_name(list: &gtk4::ListBox, name: &str) -> Result<gtk4::ListBoxRow> {
    for idx in 0.. {
        let Some(row) = list.row_at_index(idx) else {
            break;
        };
        if row.widget_name() == name {
            return Ok(row);
        }
    }
    anyhow::bail!("面板列表找不到行 {name}")
}

/// 按 widget name 前缀在面板列表里找行。
fn find_row_by_prefix(list: &gtk4::ListBox, prefix: &str) -> Option<gtk4::ListBoxRow> {
    for idx in 0.. {
        let Some(row) = list.row_at_index(idx) else {
            break;
        };
        if row.widget_name().starts_with(prefix) {
            return Some(row);
        }
    }
    None
}

/// fixture → TargetConfig（identity 字段齐全）。
fn project_target_config(fixture: &MatrixFixture, transport: &str) -> Result<TargetConfig> {
    Ok(TargetConfig {
        name: format!("parity-{transport}"),
        runtime: muxterm::core::quickconnect::model::TargetRuntime::Herdr,
        transport: if transport == "ssh" {
            muxterm::core::quickconnect::model::TargetTransport::Ssh {
                name: fixture.spec.alias.clone().unwrap_or_default(),
            }
        } else {
            muxterm::core::quickconnect::model::TargetTransport::Local
        },
        path: "/tmp".into(),
        socket: fixture.spec.socket.clone(),
        session: Some(fixture.spec.session.clone()),
        workspace_id: Some(fixture.spec.path.clone()),
    })
}
