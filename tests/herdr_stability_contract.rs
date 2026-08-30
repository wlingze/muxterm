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

use muxterm::core::catalog::Catalog;
use muxterm::core::model::task::{Task, TaskOutcome};
use muxterm::core::runtime::herdr::observe::StreamMode;
use muxterm::core::runtime::herdr::wire::{
    read_message, write_message, ClientKeybindings, ClientLaunchMode, ClientMessage,
    RenderEncoding, ServerMessage, HERDR_PROTOCOL_VERSION, MAX_FRAME_SIZE,
};
use muxterm::core::runtime::HerdrRuntime;
use muxterm::core::types::PaneId;
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

/// 核心场景：只 active pane 持有 controller，其余 observe；takeover 后有界。
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
    let mut catalog = Catalog::with_builtins();
    let workspace = rt.block_on(catalog.open(&spec))?;
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

    // 3) 连续 focus 所有 pane：每次只有新 active 是 Control，旧 active 降 Observe。
    for pane in &all_panes {
        if *pane == first_active {
            continue;
        }
        switch_pane(workspace, *pane, &format!("focus {pane}"))?;
        wait_actual_mode(workspace, *pane, StreamMode::Control, "新 active → Control")?;
        wait_actual_mode(
            workspace,
            first_active,
            StreamMode::Observe,
            "旧 active → Observe",
        )?;
        let wire = herdr_runtime(workspace)?
            .test_herdr_pane_id(*pane)
            .context("pane 缺 wire id")?
            .to_string();
        ensure!(
            !raw_control_attempt(&client_socket, &wire, false)?,
            "新 active pane 的 raw takeover=false 必须被拒"
        );
        // 每次最多一个 controller：旧 active 现在应能被接管。
        let old_wire = herdr_runtime(workspace)?
            .test_herdr_pane_id(first_active)
            .context("旧 active 缺 wire id")?
            .to_string();
        ensure!(
            raw_control_attempt(&client_socket, &old_wire, false)?,
            "每次最多一个 controller：旧 active 应已释放控制权"
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
