//! Herdr local/SSH 的服务端焦点、Workspace 与 GTK/VTE 三方一致性。
//!
//! 所有 UI 操作走生产 Action 与 VTE commit。每次 tab/pane 切换后都读取
//! Herdr `session.snapshot`；每个命令 token 同时检查服务端 `pane.read`、
//! Workspace PaneBuf/search 与唯一目标 VTE。

#![cfg(feature = "gtk")]

mod support;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use gtk4::prelude::*;

use muxterm::core::config::{Action, Config};
use muxterm::core::runtime::HerdrSession;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::linux_gtk::{gtk_test_framework_smoke, load_theme, pump_main_loop, skip_no_display};
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

fn tick(app: &AppWindow) {
    app.test_poll_once();
    pump_main_loop(25);
    app.test_flush_feeds();
}

fn wait_for(app: &AppWindow, label: &str, predicate: impl Fn(&AppWindow) -> bool) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        tick(app);
        if predicate(app) {
            return Ok(());
        }
    }
    anyhow::bail!(
        "等待 {label} 超时: tab={} pane={} leaves={:?} ids={:?}",
        app.test_active_tab_id(),
        app.test_active_pane_id(),
        app.test_layout_leaf_ids(),
        app.test_workspace_replica_ids()
    )
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
        .context("服务端 active tab 缺 layout/focused pane")?;
    ensure!(
        snapshot.focused_workspace_id.as_deref() == Some(workspace_id),
        "服务端 focused_workspace_id={:?}, expected={workspace_id}",
        snapshot.focused_workspace_id
    );
    ensure!(
        snapshot.focused_tab_id.as_deref() == Some(tab.as_str()),
        "服务端 focused_tab_id={:?}, expected={tab}",
        snapshot.focused_tab_id
    );
    ensure!(
        snapshot.focused_pane_id.as_deref() == Some(pane.as_str()),
        "服务端 focused_pane_id={:?}, expected={pane}",
        snapshot.focused_pane_id
    );
    Ok((tab, pane))
}

fn assert_three_party_focus(
    app: &AppWindow,
    authority: Authority<'_>,
    target: FocusTarget<'_>,
    label: &str,
) -> Result<()> {
    ensure!(
        app.test_active_tab_id() == target.product_tab,
        "{label}: GTK/Workspace active tab={}, expected={}",
        app.test_active_tab_id(),
        target.product_tab
    );
    ensure!(
        app.test_active_pane_id() == target.product_pane,
        "{label}: GTK/Workspace active pane={}, expected={}",
        app.test_active_pane_id(),
        target.product_pane
    );
    ensure!(
        app.test_layout_leaf_ids().contains(&target.product_pane),
        "{label}: active pane 不在当前 GTK layout"
    );
    let (server_tab, server_pane) = server_focus(authority.session, authority.workspace_id)?;
    ensure!(
        server_tab == target.wire_tab,
        "{label}: server tab={server_tab}, expected={}",
        target.wire_tab
    );
    ensure!(
        server_pane == target.wire_pane,
        "{label}: server pane={server_pane}, expected={}",
        target.wire_pane
    );
    Ok(())
}

fn emit_command(app: &AppWindow, command: &str) -> Result<()> {
    for character in command.chars() {
        ensure!(
            app.test_emit_active_pane_commit(&character.to_string()),
            "active VTE commit 路径不存在"
        );
    }
    ensure!(
        app.test_emit_active_pane_commit("\r"),
        "active VTE Enter 路径不存在"
    );
    Ok(())
}

fn execute_and_assert_three_way_binding(
    app: &AppWindow,
    session: &HerdrSession,
    target_product: u32,
    target_wire: &str,
    pane_map: &HashMap<u32, String>,
    suffix: &str,
) -> Result<()> {
    let token = format!("HERDR_GUI_AUTH_{suffix}");
    let command = format!("printf 'HERDR_GUI_AUTH_%s\\n' '{suffix}'");
    ensure!(!command.contains(&token), "输入命令不得原样包含期望 token");
    ensure!(
        app.test_search_workspace(&token).is_empty(),
        "执行前 Workspace 不得已有 token"
    );
    emit_command(app, &command)?;

    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        tick(app);
        let server_has = session
            .pane_read_ansi(target_wire)
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(&token));
        let hits = app.test_search_workspace(&token);
        let workspace_has = hits
            .iter()
            .any(|(_, pane, line)| *pane == target_product && line.contains(&token));
        let vte_has = app.test_pane_vte_text(target_product).contains(&token);
        if server_has && workspace_has && vte_has {
            break;
        }
    }

    ensure!(
        String::from_utf8_lossy(&session.pane_read_ansi(target_wire)?).contains(&token),
        "服务端 pane.read({target_wire}) 缺 {token}"
    );
    let hit_panes = app
        .test_search_workspace(&token)
        .into_iter()
        .map(|(_, pane, _)| pane)
        .collect::<HashSet<_>>();
    ensure!(
        hit_panes == HashSet::from([target_product]),
        "Workspace token {token} 归属错误: {hit_panes:?}"
    );
    ensure!(
        app.test_pane_vte_text(target_product).contains(&token),
        "目标 VTE {target_product} 缺 {token}"
    );
    for (product, wire) in pane_map {
        if *product == target_product {
            continue;
        }
        ensure!(
            !String::from_utf8_lossy(&session.pane_read_ansi(wire)?).contains(&token),
            "服务端 token {token} 串到 {wire}"
        );
        ensure!(
            !app.test_pane_vte_text(*product).contains(&token),
            "VTE token {token} 串到产品 pane {product}"
        );
    }
    Ok(())
}

fn switch_tab_and_assert(
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
    assert_three_party_focus(app, authority, target, label)
}

fn focus_pane_with_actions(
    app: &AppWindow,
    authority: Authority<'_>,
    product_tab: u32,
    wire_tab: &str,
    target: u32,
    pane_map: &HashMap<u32, String>,
) -> Result<()> {
    for step in 0..=pane_map.len() {
        if app.test_active_pane_id() == target {
            let wire = pane_map
                .get(&target)
                .context("target product pane 缺 wire id")?;
            return assert_three_party_focus(
                app,
                authority,
                FocusTarget {
                    product_tab,
                    product_pane: target,
                    wire_tab,
                    wire_pane: wire,
                },
                &format!("聚焦目标 pane {target}"),
            );
        }
        let before = app.test_active_pane_id();
        app.test_handle_action(Action::SwitchPaneNext);
        wait_for(app, "SwitchPaneNext", |candidate| {
            candidate.test_active_pane_id() != before
        })?;
        let current = app.test_active_pane_id();
        let wire = pane_map
            .get(&current)
            .with_context(|| format!("SwitchPaneNext 到未知产品 pane {current}"))?;
        assert_three_party_focus(
            app,
            authority,
            FocusTarget {
                product_tab,
                product_pane: current,
                wire_tab,
                wire_pane: wire,
            },
            &format!("SwitchPaneNext step {step}"),
        )?;
    }
    anyhow::bail!("无法通过生产 SwitchPaneNext 聚焦 pane {target}")
}

fn run_case(sshd: &LoopbackSshd, transport: &str) -> Result<()> {
    let herdr = IsolatedHerdr::start(&format!("gtk-authority-{transport}"));
    let (workspace_id, wire_tab1, wire_pane1) =
        herdr.create_workspace("/tmp", &format!("gtk-authority-{transport}"));
    let session = HerdrSession::new(herdr.name(), herdr.socket_path());
    let authority = Authority {
        session: &session,
        workspace_id: &workspace_id,
    };
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
    app.test_open_spec(spec);
    wait_for(&app, "Herdr attach", |candidate| {
        candidate.test_tab_ids().len() == 1 && candidate.test_layout_leaf_ids().len() == 1
    })?;
    let product_tab1 = app.test_active_tab_id();
    let product_pane1 = app.test_active_pane_id();
    assert_three_party_focus(
        &app,
        authority,
        FocusTarget {
            product_tab: product_tab1,
            product_pane: product_pane1,
            wire_tab: &wire_tab1,
            wire_pane: &wire_pane1,
        },
        "attach",
    )?;

    app.test_handle_action(Action::NewTab);
    wait_for(&app, "创建 Tab 2", |candidate| {
        candidate.test_tab_ids().len() == 2 && candidate.test_active_tab_id() != product_tab1
    })?;
    let product_tab2 = app.test_active_tab_id();
    let product_left = app.test_active_pane_id();
    let (wire_tab2, wire_left) = server_focus(&session, &workspace_id)?;
    assert_three_party_focus(
        &app,
        authority,
        FocusTarget {
            product_tab: product_tab2,
            product_pane: product_left,
            wire_tab: &wire_tab2,
            wire_pane: &wire_left,
        },
        "NewTab",
    )?;

    app.test_handle_action(Action::NewPane);
    wait_for(&app, "创建右 pane", |candidate| {
        candidate.test_layout_leaf_ids().len() == 2
            && candidate.test_active_pane_id() != product_left
    })?;
    let product_right_top = app.test_active_pane_id();
    let (server_tab, wire_right_top) = server_focus(&session, &workspace_id)?;
    ensure!(server_tab == wire_tab2, "右 split 后服务端切错 tab");
    assert_three_party_focus(
        &app,
        authority,
        FocusTarget {
            product_tab: product_tab2,
            product_pane: product_right_top,
            wire_tab: &wire_tab2,
            wire_pane: &wire_right_top,
        },
        "NewPane",
    )?;

    app.test_handle_action(Action::NewPaneVertical);
    wait_for(&app, "创建右下 pane", |candidate| {
        candidate.test_layout_leaf_ids().len() == 3
            && candidate.test_active_pane_id() != product_right_top
    })?;
    let product_right_bottom = app.test_active_pane_id();
    let (server_tab, wire_right_bottom) = server_focus(&session, &workspace_id)?;
    ensure!(server_tab == wire_tab2, "下 split 后服务端切错 tab");
    assert_three_party_focus(
        &app,
        authority,
        FocusTarget {
            product_tab: product_tab2,
            product_pane: product_right_bottom,
            wire_tab: &wire_tab2,
            wire_pane: &wire_right_bottom,
        },
        "NewPaneVertical",
    )?;
    ensure!(
        app.test_gtk_paned_orientations()
            == vec![gtk4::Orientation::Horizontal, gtk4::Orientation::Vertical],
        "GTK 必须保持 H(left,V(right-top,right-bottom))，实际 {}",
        app.test_gtk_layout_signature()
    );

    let pane_map = HashMap::from([
        (product_pane1, wire_pane1.clone()),
        (product_left, wire_left.clone()),
        (product_right_top, wire_right_top.clone()),
        (product_right_bottom, wire_right_bottom.clone()),
    ]);
    ensure!(pane_map.len() == 4, "四个产品 pane id 必须唯一");

    switch_tab_and_assert(
        &app,
        Action::SwitchTab1,
        authority,
        FocusTarget {
            product_tab: product_tab1,
            product_pane: product_pane1,
            wire_tab: &wire_tab1,
            wire_pane: &wire_pane1,
        },
        "SwitchTab1",
    )?;
    execute_and_assert_three_way_binding(
        &app,
        &session,
        product_pane1,
        &wire_pane1,
        &pane_map,
        &format!("{}_T1", transport.to_ascii_uppercase()),
    )?;

    switch_tab_and_assert(
        &app,
        Action::SwitchTab2,
        authority,
        FocusTarget {
            product_tab: product_tab2,
            product_pane: product_right_bottom,
            wire_tab: &wire_tab2,
            wire_pane: &wire_right_bottom,
        },
        "SwitchTab2",
    )?;
    for (index, product) in [product_left, product_right_top, product_right_bottom]
        .into_iter()
        .enumerate()
    {
        focus_pane_with_actions(
            &app,
            authority,
            product_tab2,
            &wire_tab2,
            product,
            &pane_map,
        )?;
        let wire = pane_map
            .get(&product)
            .context("目标 pane 缺 wire mapping")?
            .clone();
        execute_and_assert_three_way_binding(
            &app,
            &session,
            product,
            &wire,
            &pane_map,
            &format!("{}_T2_P{}", transport.to_ascii_uppercase(), index + 1),
        )?;
    }

    app.shutdown();
    Ok(())
}

#[test]
fn linux_local_and_ssh_herdr_focus_observe_and_vte_match_server_authority() {
    if skip_no_display() {
        return;
    }
    assert!(herdr_available(), "GTK Herdr authority e2e 要求 herdr");
    assert!(
        loopback_sshd_available(),
        "GTK Herdr authority e2e 要求可自启 loopback sshd"
    );

    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let sshd = LoopbackSshd::start("gtk-herdr-authority").expect("启动 loopback sshd");
        let _ssh_config = EnvRestore::set("MUXTERM_SSH_CONFIG_PATH", &sshd.config_path);
        for transport in ["local", "ssh"] {
            run_case(&sshd, transport)
                .unwrap_or_else(|error| panic!("GTK Herdr {transport} authority e2e: {error:#}"));
        }
    });
}
