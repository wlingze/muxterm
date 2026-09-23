//! Herdr stream 稳定性契约（W2）：controller/observer 绑定、takeover
//! suppression、有界自动重试。
//!
//! 服务端侧断言通过**第二个 raw client** 实现：对 active pane 发
//! `ControlTerminal{takeover:false}` 必须被拒（已有 controller）；对仅被
//! observe 的 pane 则接受。这比只读 Workspace 状态强得多——它证明服务端
//! 的真实所有权与 Muxterm 的 desired mode 一致。
//!
//! local / loopback SSH 都跑同一套场景。测试只使用 `muxterm-test-*` named
//! session；不触碰用户默认 Herdr server。

mod support;

use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};

use muxterm::test_support::core::catalog::Catalog;
use muxterm::test_support::core::muxterm::Muxterm;
use muxterm::test_support::core::protocol::task::{Task, TaskOutcome};
use muxterm::test_support::core::protocol::PaneId;
use muxterm::test_support::core::runtime::herdr::observe::StreamMode;
use muxterm::test_support::core::runtime::herdr::wire::{
    read_message, write_message, ClientKeybindings, ClientLaunchMode, ClientMessage,
    RenderEncoding, ServerMessage, HERDR_PROTOCOL_VERSION, MAX_FRAME_SIZE,
};
use muxterm::test_support::core::runtime::herdr::HerdrRuntime;
use muxterm::test_support::core::transport::registry::ConnectionRegistry;
use muxterm::test_support::core::workspace::Workspace;
use muxterm::test_support::core::workspace::WorkspacePool;
use muxterm::test_support::core::workspace::WorkspaceSpec;
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

fn switch_pane(workspace: &mut Workspace, target: PaneId, label: &str) -> Result<()> {
    done(workspace, Task::SwitchPane { target }, label)?;
    wait_until(workspace, label, |candidate| {
        candidate.state().active_pane().map(|pane| pane.id) == Some(target)
    })
}

fn active_pane(workspace: &Workspace) -> Result<PaneId> {
    workspace
        .state()
        .active_pane()
        .map(|pane| pane.id)
        .context("缺 active pane")
}

fn herdr_runtime(workspace: &Workspace) -> Result<&HerdrRuntime> {
    workspace
        .runtime()
        .as_any()
        .downcast_ref::<HerdrRuntime>()
        .context("Catalog 没有打开 HerdrRuntime")
}

/// 第二个 raw client：连接 client socket，Hello 握手后尝试
/// `ControlTerminal{takeover}`。`Ok(true)` = 服务端接受（成为 controller）；
/// `Ok(false)` = 被拒（该 pane 已有 controller）。忽略握手后的 Notify 等
/// 非 Terminal/ServerShutdown 消息。
fn raw_control_attempt(socket: &Path, target: &str, takeover: bool) -> Result<bool> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket)
        .with_context(|| format!("连接 client socket {} 失败", socket.display()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .context("设置 raw client 读超时失败")?;
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
    .context("raw client 写 Hello 失败")?;
    let _welcome: ServerMessage =
        read_message(&mut stream, MAX_FRAME_SIZE).context("raw client 读 Welcome 失败")?;
    write_message(
        &mut stream,
        &ClientMessage::ControlTerminal {
            target: target.to_string(),
            takeover,
        },
    )
    .context("raw client 写 ControlTerminal 失败")?;
    loop {
        match read_message::<_, ServerMessage>(&mut stream, MAX_FRAME_SIZE) {
            Ok(ServerMessage::Terminal(_)) => return Ok(true),
            Ok(ServerMessage::ServerShutdown { .. }) => return Ok(false),
            Ok(_) => continue,
            Err(err) => anyhow::bail!("raw client 读响应失败: {err}"),
        }
    }
}

/// 等待某 pane 的 stream 实际进入期望模式。
fn wait_actual_mode(
    workspace: &mut Workspace,
    pane: PaneId,
    mode: StreamMode,
    label: &str,
) -> Result<()> {
    wait_until(workspace, label, |candidate| {
        herdr_runtime(candidate)
            .ok()
            .and_then(|rt| rt.test_actual_mode(pane))
            == Some(mode)
    })
}

/// 旧 active pane 应保持什么模式。
///
/// 同 tab 分屏：各自保留 controller。
/// 切到其它 tab：分支上「Live Control 在 desired 变 Observe 时保留」是有意
/// 为之——拆掉再重建会让切回来整屏重绘，也会把服务端控制权交还。所以这里
/// 仍要求 Control，而不是旧契约的降级 Observe。
fn expected_previous_mode(first_visible: bool, was_live_control: bool) -> StreamMode {
    if first_visible || was_live_control {
        StreamMode::Control
    } else {
        StreamMode::Observe
    }
}

/// 核心场景：可见且有 allocation 的 pane 持有 controller；takeover 后有界。
fn run_stability_case(
    rt: &tokio::runtime::Runtime,
    sshd: &LoopbackSshd,
    transport: &str,
) -> Result<()> {
    let herdr = IsolatedHerdr::start(&format!("stability-{transport}"));
    let (workspace_id, _tab, _pane) =
        herdr.create_workspace("/tmp", &format!("stability-{transport}"));

    // 夹具：4 tab；tab1 上 split 出 3 pane。
    let tab2 = herdr_tab_create(&herdr, &workspace_id, "stab-t2");
    let tab3 = herdr_tab_create(&herdr, &workspace_id, "stab-t3");
    let tab4 = herdr_tab_create(&herdr, &workspace_id, "stab-t4");
    let p1 = herdr_pane_of_tab(&herdr, &workspace_id, &tab1_of(&herdr, &workspace_id));
    let _p2 = herdr.split_pane(&p1, "right");
    let _p3 = herdr.split_pane(&p1, "down");
    let _ = (tab2, tab3, tab4);

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
    let catalog = Catalog::with_builtins();
    let mut connections = ConnectionRegistry::new();
    let mut pool = WorkspacePool::default();
    let runtime = Muxterm::new_runtime(&catalog, &mut connections, &spec)?;
    let workspace = rt.block_on(pool.open_spec_with_runtime(&spec, runtime))?;
    wait_until(workspace, "初始 Herdr tab/pane", |ws| {
        ws.state().tabs().len() == 4 && ws.state().active_pane().is_some()
    })?;

    let runtime = herdr_runtime(workspace)?;
    // SSH：raw client 也要走 Runtime 持有的本地 forwarded client socket。
    let client_socket = runtime.session().client_socket_path().to_path_buf();
    let all_panes = workspace
        .state()
        .tabs()
        .iter()
        .flat_map(|tab| workspace.state().panes(&tab.id))
        .map(|p| p.id)
        .collect::<Vec<_>>();
    ensure!(
        all_panes.len() >= 4,
        "夹具应至少 4 pane，实际 {}",
        all_panes.len()
    );

    // 1) 初始：active pane 必须已是 Control；其它 pane 等待 Observe。
    let first_active = active_pane(workspace)?;
    // 无 GUI 的契约必须显式模拟前端 viewport；Herdr 在 preferred size
    // 到达前保持 Observe，避免默认 80×24 Control Hello 重排远端 TUI。
    workspace.execute(Task::ResizePane {
        target: first_active,
        cols: 80,
        rows: 24,
    })?;
    wait_actual_mode(
        workspace,
        first_active,
        StreamMode::Control,
        "active pane → Control",
    )?;
    for pane in &all_panes {
        if *pane != first_active {
            wait_actual_mode(
                workspace,
                *pane,
                StreamMode::Observe,
                "非 active pane → Observe",
            )?;
        }
    }

    // 2) 服务端断言：raw client 对 active pane 的 takeover=false 必须被拒；
    //    对非 active pane 必须接受。
    let first_wire = herdr_runtime(workspace)?
        .test_herdr_pane_id(first_active)
        .context("active pane 缺 wire id")?
        .to_string();
    ensure!(
        !raw_control_attempt(&client_socket, &first_wire, false)?,
        "active pane 已有 controller：raw takeover=false 必须被拒"
    );
    for pane in all_panes.iter().filter(|p| **p != first_active) {
        let wire = herdr_runtime(workspace)?
            .test_herdr_pane_id(*pane)
            .context("pane 缺 wire id")?
            .to_string();
        ensure!(
            raw_control_attempt(&client_socket, &wire, false)?,
            "非 active pane 只被 observe：raw takeover=false 应接受（pane {wire}）"
        );
    }

    // 3) 连续 focus：已有 allocation 的 first_active 在同 tab 内保留 Control；
    //    切到其它 tab 时 Live Control 也保留（避免切回来整屏重绘）；
    //    没有 allocation 的隐藏 pane 不能接管尺寸。
    let first_tab = workspace
        .state()
        .tabs()
        .iter()
        .find(|tab| {
            workspace
                .state()
                .panes(&tab.id)
                .iter()
                .any(|pane| pane.id == first_active)
        })
        .map(|tab| tab.id)
        .context("missing first tab")?;
    for pane in &all_panes {
        if *pane == first_active {
            continue;
        }
        switch_pane(workspace, *pane, &format!("focus {pane}"))?;
        wait_actual_mode(workspace, *pane, StreamMode::Control, "新 active → Control")?;
        let first_visible = workspace
            .state()
            .panes(&first_tab)
            .iter()
            .any(|p| p.id == *pane);
        let first_was_live_control =
            herdr_runtime(workspace)?.test_actual_mode(first_active) == Some(StreamMode::Control);
        wait_actual_mode(
            workspace,
            first_active,
            expected_previous_mode(first_visible, first_was_live_control),
            "previous pane follows tab visibility",
        )?;
        let wire = herdr_runtime(workspace)?
            .test_herdr_pane_id(*pane)
            .context("pane 缺 wire id")?
            .to_string();
        ensure!(
            !raw_control_attempt(&client_socket, &wire, false)?,
            "新 active pane 的 raw takeover=false 必须被拒"
        );
        // 同 tab 分屏保留各自 controller；隐藏 tab 的 Live Control 也保留
        // （分支行为），所以只有真正没有 controller 的 pane 才接受 takeover。
        let old_wire = herdr_runtime(workspace)?
            .test_herdr_pane_id(first_active)
            .context("旧 active 缺 wire id")?
            .to_string();
        let keeps_control = herdr_runtime(workspace)?.test_actual_mode(first_active)
            == Some(StreamMode::Control);
        ensure!(
            raw_control_attempt(&client_socket, &old_wire, false)? != keeps_control,
            "previous pane ownership must follow its live mode"
        );
    }

    // 4) takeover 风暴有界：对当前 active pane 连续 takeover=true 6 次，
    //    10 秒窗口内 control auto-start 不得增加、总 stream start ≤ 5。
    //    （storm 前的 control takeover 来自步骤 3 的用户 focus promote，
    //    不计入自动重试。）
    let storm_target = active_pane(workspace)?;
    let storm_wire = herdr_runtime(workspace)?
        .test_herdr_pane_id(storm_target)
        .context("storm pane 缺 wire id")?
        .to_string();
    let starts_before = herdr_runtime(workspace)?.test_stream_starts(storm_target);
    let takeover_before_storm =
        herdr_runtime(workspace)?.test_control_takeover_starts(storm_target);
    let window_start = Instant::now();
    for round in 0..6 {
        ensure!(
            raw_control_attempt(&client_socket, &storm_wire, true)?,
            "takeover=true 必须被接受（round {round}）"
        );
        // 每次 takeover 后等 suppression 生效（旧 control 被服务端踢掉）。
        wait_until(workspace, "takeover suppression", |candidate| {
            herdr_runtime(candidate)
                .ok()
                .is_some_and(|rt| rt.test_takeover_suppressed(storm_target))
        })?;
        // 再等 Observe demote 完成。
        wait_actual_mode(workspace, storm_target, StreamMode::Observe, "降 Observe")?;
    }
    ensure!(
        window_start.elapsed() <= Duration::from_secs(10),
        "takeover 场景应在 10 秒窗口内完成"
    );
    let runtime = herdr_runtime(workspace)?;
    ensure!(
        runtime.test_control_takeover_starts(storm_target) == takeover_before_storm,
        "taken-over 后无用户动作时 control auto-start 必须为 0（不得增加）：before={takeover_before_storm}, after={}",
        runtime.test_control_takeover_starts(storm_target)
    );
    ensure!(
        runtime.test_stream_starts(storm_target) - starts_before <= 5,
        "10 秒内自动 start ≤5：before={starts_before}, after={}",
        runtime.test_stream_starts(storm_target)
    );

    // 5) 重复 snapshot/reconciliation 不能清除 suppression，不能反抢 control。
    for _ in 0..5 {
        let _ = workspace.refresh();
    }
    let runtime = herdr_runtime(workspace)?;
    ensure!(
        runtime.test_takeover_suppressed(storm_target),
        "重复 reconciliation 不得清除 takeover suppression"
    );
    ensure!(
        runtime.test_control_takeover_starts(storm_target) == takeover_before_storm,
        "重复 reconciliation 不得自动反抢 control"
    );

    // 6) 用户再次 focus/input：只 promote 一次（takeover=true），并可输出 token。
    switch_pane(workspace, storm_target, "用户重新 focus storm pane")?;
    wait_until(workspace, "promote 后 Control", |candidate| {
        herdr_runtime(candidate)
            .ok()
            .and_then(|rt| rt.test_actual_mode(storm_target))
            == Some(StreamMode::Control)
    })?;
    let runtime = herdr_runtime(workspace)?;
    ensure!(
        runtime.test_control_takeover_starts(storm_target) == takeover_before_storm + 1,
        "用户 focus 后只 promote 一次，实际 {}（before={takeover_before_storm}）",
        runtime.test_control_takeover_starts(storm_target)
    );
    ensure!(
        !runtime.test_takeover_suppressed(storm_target),
        "新用户 intent 必须清除 suppression"
    );

    // 输入 token 恰好一次送达目标 pane。
    let token = format!("STAB_{transport}_PROMOTE");
    let command = format!("printf 'STAB_%s\\n' '{transport}_PROMOTE'\r");
    ensure!(!command.contains(&token), "输入命令不得原样包含期望 token");
    done(
        workspace,
        Task::WriteRaw {
            target: storm_target,
            data: command.into_bytes(),
        },
        "promote 后输入 token",
    )?;
    wait_until(workspace, "token 到达 Workspace", |candidate| {
        candidate.search_workspace(&token).len() == 1
    })?;

    rt.block_on(workspace.shutdown())?;
    Ok(())
}

fn herdr_tab_create(herdr: &IsolatedHerdr, workspace_id: &str, label: &str) -> String {
    let out = herdr
        .cli()
        .args([
            "tab",
            "create",
            "--workspace",
            workspace_id,
            "--label",
            label,
        ])
        .output()
        .expect("tab create 失败");
    assert!(
        out.status.success(),
        "tab create 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("tab create 输出不是 JSON");
    v["result"]["tab"]["tab_id"]
        .as_str()
        .or_else(|| v["result"]["tab_id"].as_str())
        .expect("tab create 缺 tab_id")
        .to_string()
}

/// 找 workspace 的第一个 tab 的 public id。
fn tab1_of(herdr: &IsolatedHerdr, workspace_id: &str) -> String {
    let out = herdr
        .cli()
        .args(["tab", "list", "--workspace", workspace_id])
        .output()
        .expect("tab list 失败");
    assert!(
        out.status.success(),
        "tab list 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("tab list 输出不是 JSON");
    v["result"]["tabs"][0]["tab_id"]
        .as_str()
        .expect("tab list 缺 tab_id")
        .to_string()
}

/// 某 tab 的第一个 pane 的 public id。
fn herdr_pane_of_tab(herdr: &IsolatedHerdr, workspace_id: &str, tab_id: &str) -> String {
    let out = herdr
        .cli()
        .args(["pane", "list"])
        .output()
        .expect("pane list 失败");
    assert!(
        out.status.success(),
        "pane list 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("pane list 输出不是 JSON");
    v["result"]["panes"]
        .as_array()
        .expect("pane list 缺 panes")
        .iter()
        .find(|p| p["workspace_id"] == workspace_id && p["tab_id"] == tab_id)
        .and_then(|p| p["pane_id"].as_str())
        .expect("pane list 缺 pane_id")
        .to_string()
}

#[test]
fn local_and_ssh_herdr_stream_stability_contract() {
    assert!(herdr_available(), "Herdr stability contract 要求 herdr");
    assert!(
        loopback_sshd_available(),
        "Herdr stability contract 要求可自启 loopback sshd"
    );
    let sshd = LoopbackSshd::start("herdr-stability").expect("启动 loopback sshd");
    let _ssh_config = EnvRestore::set("MUXTERM_SSH_CONFIG_PATH", &sshd.config_path);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("Tokio runtime");

    for transport in ["local", "ssh"] {
        run_stability_case(&rt, &sshd, transport)
            .unwrap_or_else(|error| panic!("Herdr {transport} stability contract: {error:#}"));
    }
}

/// 可见分屏的非焦点 pane 也必须拥有实际 PTY 尺寸，不能只扩大 observer 画布。
#[test]
fn visible_split_panes_keep_independent_pty_sizes() -> Result<()> {
    ensure!(herdr_available(), "this regression requires Herdr 0.8.0");
    let executor = tokio::runtime::Runtime::new()?;
    let _entered = executor.enter();
    let herdr = IsolatedHerdr::start("split-size");
    let (workspace_id, _, upper_wire) = herdr.create_workspace("/tmp", "split-size");
    let lower_wire = herdr.split_pane(&upper_wire, "down");
    let spec = WorkspaceSpec::herdr(
        herdr.name(),
        &workspace_id,
        herdr.socket_path().to_string_lossy(),
    );
    let catalog = Catalog::with_builtins();
    let mut connections = ConnectionRegistry::new();
    let mut pool = WorkspacePool::default();
    let runtime = Muxterm::new_runtime(&catalog, &mut connections, &spec)?;
    let workspace = executor.block_on(pool.open_spec_with_runtime(&spec, runtime))?;
    let find_pane = |wire: &str| -> Result<PaneId> {
        workspace
            .state()
            .tabs()
            .iter()
            .flat_map(|tab| workspace.state().panes(&tab.id))
            .find(|pane| {
                herdr_runtime(workspace)
                    .unwrap()
                    .test_herdr_pane_id(pane.id)
                    == Some(wire)
            })
            .map(|pane| pane.id)
            .context("missing split pane")
    };
    let upper = find_pane(&upper_wire)?;
    let lower = find_pane(&lower_wire)?;
    switch_pane(workspace, lower, "focus lower split")?;
    for (pane, cols, rows) in [(upper, 178, 21), (lower, 178, 23)] {
        done(
            workspace,
            Task::ResizePane {
                target: pane,
                cols,
                rows,
            },
            "allocate split",
        )?;
    }
    wait_actual_mode(workspace, lower, StreamMode::Control, "lower controller")?;
    wait_actual_mode(workspace, upper, StreamMode::Control, "upper controller")?;
    // 从实际 shell 的 stty 读取 PTY 尺寸，而不是检查 observer frame 的宽高。
    let session = herdr_runtime(workspace)?.session_arc().clone();
    for (wire, rows, cols) in [(&upper_wire, 21, 178), (&lower_wire, 23, 178)] {
        session.pane_send_text(wire, "stty size\r")?;
        let expected = format!("{rows} {cols}");
        wait_until(workspace, &format!("PTY {wire} = {expected}"), |_| {
            session
                .pane_read_recent_ansi(wire)
                .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(&expected))
        })?;
    }
    let generation = herdr_runtime(workspace)?.test_stream_starts(upper);
    switch_pane(workspace, upper, "focus upper split")?;
    switch_pane(workspace, lower, "return to lower split")?;
    ensure!(
        herdr_runtime(workspace)?.test_stream_starts(upper) == generation,
        "focus within the same visible tab must not restart the upper stream"
    );
    done(
        workspace,
        Task::ResizePane {
            target: upper,
            cols: 132,
            rows: 17,
        },
        "resize unfocused upper",
    )?;
    session.pane_send_text(&upper_wire, "stty size\r")?;
    wait_until(workspace, "resized upper PTY = 17 132", |_| {
        session
            .pane_read_recent_ansi(&upper_wire)
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains("17 132"))
    })?;
    // 外部接管非焦点 pane 后，后续布局 resize 不得抢回 controller。
    let client_socket = session.client_socket_path();
    ensure!(
        raw_control_attempt(client_socket, &upper_wire, true)?,
        "external takeover"
    );
    wait_until(workspace, "upper takeover suppression", |ws| {
        herdr_runtime(ws).is_ok_and(|runtime| runtime.test_takeover_suppressed(upper))
    })?;
    wait_actual_mode(
        workspace,
        upper,
        StreamMode::Observe,
        "upper yields ownership",
    )?;
    let starts = herdr_runtime(workspace)?.test_stream_starts(upper);
    done(
        workspace,
        Task::ResizePane {
            target: upper,
            cols: 140,
            rows: 19,
        },
        "resize suppressed pane",
    )?;
    for _ in 0..5 {
        workspace.refresh();
    }
    ensure!(
        herdr_runtime(workspace)?.test_takeover_suppressed(upper),
        "resize must not clear suppression"
    );
    ensure!(
        herdr_runtime(workspace)?.test_stream_starts(upper) == starts,
        "resize must not reacquire control"
    );
    switch_pane(workspace, upper, "explicit focus rearms upper")?;
    wait_actual_mode(
        workspace,
        upper,
        StreamMode::Control,
        "explicit focus restores control",
    )?;
    Ok(())
}

#[test]
fn server_scroll_reaches_history_before_and_after_attach() -> Result<()> {
    let executor = tokio::runtime::Runtime::new()?;
    let _entered = executor.enter();
    let herdr = IsolatedHerdr::start("scroll");
    let (workspace_id, _, wire) = herdr.create_workspace("/tmp", "scroll");
    let session = muxterm::test_support::core::runtime::herdr::session::HerdrSession::new(
        herdr.name(),
        herdr.socket_path(),
    );
    session.pane_send_text(
        &wire,
        "for i in $(seq 1 150); do printf 'HISTORY-%s\\n' \"$i\"; done\r",
    )?;
    let spec = WorkspaceSpec::herdr(
        herdr.name(),
        &workspace_id,
        herdr.socket_path().to_string_lossy(),
    );
    let catalog = Catalog::with_builtins();
    let mut connections = ConnectionRegistry::new();
    let mut pool = WorkspacePool::default();
    let runtime = Muxterm::new_runtime(&catalog, &mut connections, &spec)?;
    let workspace = executor.block_on(pool.open_spec_with_runtime(&spec, runtime))?;
    let pane = active_pane(workspace)?;
    done(
        workspace,
        Task::ResizePane {
            target: pane,
            cols: 100,
            rows: 24,
        },
        "allocate",
    )?;
    wait_actual_mode(workspace, pane, StreamMode::Control, "controller")?;
    wait_until(workspace, "history populated", |_| {
        session
            .pane_read_recent_ansi_lines(&wire, 200)
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains("HISTORY-150"))
    })?;
    for after_attach in [false, true] {
        if after_attach {
            session.pane_send_text(
                &wire,
                "for i in $(seq 1 100); do printf 'NEW-%s\\n' \"$i\"; done\r",
            )?;
            wait_until(workspace, "new output", |_| {
                session
                    .pane_read_recent_ansi(&wire)
                    .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains("NEW-100"))
            })?;
        }
        done(
            workspace,
            Task::ScrollPane {
                target: pane,
                lines: 30,
            },
            "scroll up",
        )?;
        wait_until(workspace, "server viewport moves into history", |_| {
            session.snapshot().is_ok_and(|snapshot| {
                snapshot.panes.iter().any(|p| {
                    p.pane_id == wire
                        && p.scroll
                            .as_ref()
                            .is_some_and(|s| s.offset_from_bottom >= 30)
                })
            })
        })?;
        done(
            workspace,
            Task::ScrollPane {
                target: pane,
                lines: -65535,
            },
            "scroll back down",
        )?;
        wait_until(workspace, "server viewport returns to latest", |_| {
            session.snapshot().is_ok_and(|snapshot| {
                snapshot.panes.iter().any(|p| {
                    p.pane_id == wire
                        && p.scroll.as_ref().is_some_and(|s| s.offset_from_bottom == 0)
                })
            })
        })?;
    }
    Ok(())
}

#[test]
fn cached_tab_switches_do_not_publish_smaller_control_frames() -> Result<()> {
    use muxterm::test_support::core::protocol::state::StateChange;
    let executor = tokio::runtime::Runtime::new()?;
    let _entered = executor.enter();
    let herdr = IsolatedHerdr::start("switch-grid");
    let (workspace_id, _, _) = herdr.create_workspace("/tmp", "switch-grid");
    herdr_tab_create(&herdr, &workspace_id, "second");
    let spec = WorkspaceSpec::herdr(
        herdr.name(),
        &workspace_id,
        herdr.socket_path().to_string_lossy(),
    );
    let catalog = Catalog::with_builtins();
    let mut connections = ConnectionRegistry::new();
    let mut pool = WorkspacePool::default();
    let runtime = Muxterm::new_runtime(&catalog, &mut connections, &spec)?;
    let workspace = executor.block_on(pool.open_spec_with_runtime(&spec, runtime))?;
    let tabs: Vec<_> = workspace.state().tabs().iter().map(|tab| tab.id).collect();
    let mut panes = Vec::new();
    for tab in &tabs {
        done(workspace, Task::SwitchTab { target: *tab }, "initial tab")?;
        let pane = active_pane(workspace)?;
        panes.push(pane);
        done(
            workspace,
            Task::ResizePane {
                target: pane,
                cols: 178,
                rows: 50,
            },
            "allocate",
        )?;
        wait_until(workspace, "allocated frame", |ws| {
            ws.state()
                .active_pane()
                .is_some_and(|p| p.cols == 178 && p.rows == 50)
        })?;
    }
    for tab in tabs.iter().cycle().take(6) {
        done(workspace, Task::SwitchTab { target: *tab }, "cached switch")?;
        let deadline = Instant::now() + Duration::from_millis(350);
        while Instant::now() < deadline {
            for event in workspace.refresh() {
                if let StateChange::PaneResized { pane, cols, rows } = event {
                    ensure!(
                        !panes.contains(&pane) || (cols, rows) == (178, 50),
                        "switch shrank pane {pane}: {cols}x{rows}"
                    );
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}
