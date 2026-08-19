//! W20d/e：已有的连接发现契约。
//!
//! - W20d：IsolatedHerdr 的 workspace 出现在本地 discover，且不含用户默认 w2。
//! - W20e：LoopbackSshd 上远端 tmux session 与 Herdr workspace 都能列出。
//!
//! 无 sshd / 无 herdr 才 eprintln skip；禁止 #[ignore]。

mod support;

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use muxterm::core::attention::signal::AttentionSignal;
use muxterm::core::attention::state::PaneStatus;
use muxterm::core::catalog::Catalog;
use muxterm::core::discovery::existing::{
    discover_local_herdr, discover_ssh_herdr, discover_ssh_tmux,
};
use muxterm::core::model::state::{PaneAgentSessionKind, PaneAgentStatus, StateChange};
use muxterm::core::model::task::Task;
use muxterm::core::quickconnect::model::TargetRuntime;
use muxterm::core::runtime::HerdrRuntime;
use muxterm::core::workspace::id::WorkspaceId;
use muxterm::core::workspace::workspace::Workspace;
use support::herdr_test_support::{herdr_available, IsolatedHerdr, TempAgentCommand};
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};
use support::tmux_test_support::{create_session, kill_server, tmux_available, unique_socket};

// 本文件的夹具会修改 MUXTERM_SSH_CONFIG_PATH / HERDR_SOCKET_PATH 等
// 进程级环境变量。Rust 默认并行跑 #[test]，必须串行持有这些变量，否则
// 一个测试会让另一个 Catalog 读到错误的 SSH alias。
static PROCESS_ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_process_env() -> MutexGuard<'static, ()> {
    PROCESS_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// W20d：本地 Herdr discover 必须看到测试 workspace，且不得出现用户默认 w2。
#[test]
fn discover_local_herdr_sees_isolated_workspace_only() {
    let _env_lock = lock_process_env();
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let herdr = IsolatedHerdr::start("disc");
    let (ws, _tab, _pane) = herdr.create_workspace("/tmp", "mux-w20d");

    // 注入测试 socket；config_dir 指向空临时目录，避免扫到用户默认。
    std::env::set_var(
        "HERDR_SOCKET_PATH",
        herdr.socket_path().to_string_lossy().to_string(),
    );
    let tmp = std::env::temp_dir().join(format!(
        "muxterm-test-herdr-disc-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let entries = discover_local_herdr(Some(&tmp));
    std::env::remove_var("HERDR_SOCKET_PATH");
    let _ = std::fs::remove_dir_all(&tmp);

    assert!(
        entries
            .iter()
            .any(|e| e.herdr_workspace_id.as_deref() == Some(ws.as_str())),
        "本地 discover 必须看到刚 create 的 workspace {ws}: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .all(|e| e.herdr_workspace_id.as_deref() != Some("w2")),
        "不得出现用户默认 w2: {entries:?}"
    );
    assert!(
        entries.iter().all(|e| e.runtime == TargetRuntime::Herdr),
        "本地 discover 只应有 Herdr 行"
    );
}

/// W20e：LoopbackSshd 上远端 tmux + Herdr 都能列出。
#[test]
fn ssh_discover_lists_remote_tmux_and_herdr() {
    let _env_lock = lock_process_env();
    if !loopback_sshd_available() {
        eprintln!("skip: 无 sshd 二进制，无法自启 loopback sshd");
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux 二进制");
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let sshd = LoopbackSshd::start("existing-ssh").expect("启动 loopback sshd 失败");
    sshd.apply_ssh_config_env();

    // 远端（loopback 同机）隔离 tmux session。
    let socket = unique_socket("existing-ssh-tmux");
    create_session(&socket, "existing-ssh-sess", 80, 24);
    let tmux_guard = TmuxGuard {
        socket: socket.clone(),
    };

    // 远端（loopback 同机）隔离 Herdr named session。
    let herdr = IsolatedHerdr::start("ssh-disc");
    let (ws, _tab, _pane) = herdr.create_workspace("/tmp", "mux-w20e");

    let timeout = Duration::from_secs(5);
    let tmux_entries = discover_ssh_tmux(
        &sshd.alias,
        Some(&sshd.config_path.to_string_lossy()),
        Some(&socket),
        timeout,
    );
    assert!(
        tmux_entries
            .iter()
            .any(|e| e.tmux_session.as_deref() == Some("existing-ssh-sess")),
        "远端 tmux session 必须列出: {tmux_entries:?}"
    );

    let herdr_entries = discover_ssh_herdr(
        &sshd.alias,
        Some(&sshd.config_path.to_string_lossy()),
        timeout,
    );
    assert!(
        herdr_entries
            .iter()
            .any(|e| e.herdr_workspace_id.as_deref() == Some(ws.as_str())),
        "远端 Herdr workspace 必须列出: {herdr_entries:?}"
    );
    assert!(
        herdr_entries
            .iter()
            .all(|e| e.runtime == TargetRuntime::Herdr),
        "SSH discover 的 Herdr 行 runtime 必须是 Herdr"
    );
    drop(tmux_guard);
}

/// W20h SSH：远端 herdr.sock 转发到本机后，HerdrSession 能 attach。
#[test]
fn ssh_herdr_forward_attach_contract() {
    let _env_lock = lock_process_env();
    if !loopback_sshd_available() {
        eprintln!("skip: 无 sshd 二进制，无法自启 loopback sshd");
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let sshd = LoopbackSshd::start("herdr-fwd").expect("启动 loopback sshd 失败");
    sshd.apply_ssh_config_env();
    let agent_command = TempAgentCommand::pi("ssh-forward");
    let herdr = IsolatedHerdr::start("fwd-attach");
    let (ws, _tab, pane) = herdr.create_workspace("/tmp", "mux-fwd");

    let (local_socket, forward) = muxterm::core::runtime::herdr::forward::start_herdr_ssh_forward(
        &sshd.alias,
        &herdr.socket_path().to_string_lossy(),
        Some(&sshd.config_path.to_string_lossy()),
    )
    .expect("ssh socket 转发应就绪");

    let session = Arc::new(muxterm::core::runtime::HerdrSession::new(
        herdr.name(),
        &local_socket,
    ));
    session.ping().expect("转发后的 HerdrSession 应能 ping");
    let snap = session.snapshot().expect("转发后的 snapshot 应成功");
    assert!(
        snap.workspaces.iter().any(|w| w.workspace_id == ws),
        "转发后必须看到远端 workspace {ws}"
    );

    // 走产品 Runtime 的逐字 WriteRaw + 单独 Enter。引号把输入回显中的 token
    // 隔开；只有远端 shell 真执行 echo，双 socket observe 才会返回连续 token。
    let client_socket = session.client_socket_path().to_path_buf();
    let runtime = HerdrRuntime::with_forward(Arc::clone(&session), &ws, forward);
    let mut workspace = Workspace::new(
        WorkspaceId::new("ssh", Some(&sshd.alias), herdr.name(), "herdr", &ws),
        "ssh-herdr-input".to_string(),
        Box::new(runtime),
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(workspace.connect())
        .expect("SSH Herdr Runtime connect 应成功");
    let active = workspace
        .state()
        .active_pane()
        .expect("SSH Herdr 应有 active pane")
        .id;
    let command = "echo HERDR_EXEC_\"SSH\"";
    let output_token = "HERDR_EXEC_SSH";
    assert!(!command.contains(output_token));
    assert!(workspace.search_workspace(output_token).is_empty());
    for byte in command.bytes().chain(std::iter::once(b'\r')) {
        workspace
            .execute(Task::WriteRaw {
                target: active,
                data: vec![byte],
            })
            .expect("SSH Herdr 逐字 WriteRaw 应成功");
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let _ = workspace.refresh();
        if !workspace.search_workspace(output_token).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let hits = workspace.search_workspace(output_token);
    let pane_output = workspace
        .state()
        .pane_output(&active)
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .unwrap_or_default();
    assert!(
        !hits.is_empty(),
        "SSH Herdr 逐字输入 + Enter 必须真的执行远端命令。token={output_token} pane={pane} output={pane_output:?}"
    );

    // 同一条 SSH API forward 上启动真实 pi，并通过结构化 API 报告完整
    // agent metadata；Runtime 必须把远端 wire 统一成 PaneAgentChanged。
    let agent_executable = agent_command.cwd().join("pi");
    session
        .pane_send_text(&pane, &agent_executable.to_string_lossy())
        .expect("SSH forward 应能启动真实 pi");
    session
        .pane_send_keys(&pane, &["enter".to_string()])
        .expect("SSH forward 应能提交 pi 命令");
    let detection_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let snapshot = session.snapshot().expect("SSH forward 读取 agent snapshot");
        if snapshot
            .agents
            .iter()
            .any(|agent| agent.pane_id == pane && agent.agent.as_deref() == Some("pi"))
        {
            break;
        }
        assert!(
            Instant::now() < detection_deadline,
            "SSH forward 未识别真实 pi: {snapshot:#?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // 先让已连接的 Runtime 明确消费 screen detector 状态。若把 detector
    // 与后面的 hook report 放在同一个 poll 里，authority handoff 是否成为
    // 首个可见事件取决于线程调度，测试就会错误地要求固定 initial 值。
    let detector_deadline = Instant::now() + Duration::from_secs(15);
    let mut detector_event = false;
    loop {
        let events = workspace.refresh();
        detector_event |= events.iter().any(|event| {
            matches!(
                event,
                StateChange::PaneAgentChanged {
                    pane,
                    agent: Some(agent),
                    initial: false,
                } if *pane == active
                    && agent.status == PaneAgentStatus::Working
                    && !agent.screen_detection_skipped
            )
        });
        let complete = workspace.pane_agent(active).is_some_and(|agent| {
            agent.status == PaneAgentStatus::Working && !agent.screen_detection_skipped
        });
        if detector_event && complete {
            break;
        }
        assert!(
            Instant::now() < detector_deadline,
            "SSH screen detector Working 未进入 Workspace: {:?}",
            workspace.pane_agent(active)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(workspace
        .take_attention_signals(active)
        .iter()
        .any(|signal| {
            matches!(
                signal,
                AttentionSignal::AuthoritativeStatus {
                    status: PaneStatus::Working,
                    initial: false,
                }
            )
        }));

    let source = "herdr:pi";
    let agent_session_path = agent_command.cwd().join("pi-session.jsonl");
    std::fs::write(&agent_session_path, "{}\n").expect("创建 SSH pi session fixture");
    session
        .call(
            "pane.report_agent_session",
            serde_json::json!({
                "pane_id": pane,
                "source": source,
                "agent": "pi",
                "seq": 1,
                "agent_session_path": agent_session_path,
                "session_start_source": "startup",
            }),
        )
        .expect("SSH forward 报告 agent session");
    let session_snapshot = session
        .snapshot()
        .expect("SSH forward 读取 agent session snapshot");
    assert!(
        session_snapshot.agents.iter().any(|agent| {
            agent.pane_id == pane
                && agent.agent_session.as_ref().is_some_and(|session| {
                    session.kind == "path"
                        && session.value == agent_session_path.to_string_lossy().as_ref()
                })
        }),
        "SSH API snapshot 必须保留 agent session: {session_snapshot:#?}"
    );

    session
        .call(
            "pane.report_agent",
            serde_json::json!({
                "pane_id": pane,
                "source": source,
                "agent": "pi",
                "state": "working",
                "message": "testing SSH runtime normalization",
                "seq": 2,
                "agent_session_path": agent_session_path,
            }),
        )
        .expect("SSH forward 报告 working");
    session
        .call(
            "pane.report_metadata",
            serde_json::json!({
                "pane_id": pane,
                "source": "muxterm-test:ssh-display",
                "agent": "pi",
                "applies_to_source": source,
                "title": "SSH runtime agent",
                "display_agent": "Pi over SSH",
                "state_labels": { "blocked": "Remote approval" },
                "tokens": { "transport": "ssh", "context": "41%" },
                "seq": 1,
            }),
        )
        .expect("SSH forward 报告 agent metadata");

    // Working 与 session-only report 都不改变 detector 已有的 Working 状态；
    // metadata 的 pane.updated 让 Runtime 看到完整 hook authority。这个
    // detector -> hook 切换是 bootstrap，必须携带完整 metadata/session，
    // 但不能制造用户通知。
    let handoff_deadline = Instant::now() + Duration::from_secs(15);
    let mut handoff_event = false;
    loop {
        let events = workspace.refresh();
        handoff_event |= events.iter().any(|event| {
            matches!(
                event,
                StateChange::PaneAgentChanged {
                    pane,
                    agent: Some(agent),
                    initial: true,
                } if *pane == active
                    && agent.status == PaneAgentStatus::Working
                    && agent.screen_detection_skipped
                    && agent.display_name.as_deref() == Some("Pi over SSH")
                    && agent.session.as_ref().is_some_and(|session| {
                        session.kind == PaneAgentSessionKind::Path
                    })
            )
        });
        let complete = workspace.pane_agent(active).is_some_and(|agent| {
            agent.status == PaneAgentStatus::Working
                && agent.screen_detection_skipped
                && agent.display_name.as_deref() == Some("Pi over SSH")
                && agent.tokens.get("transport").map(String::as_str) == Some("ssh")
                && agent.session.as_ref().is_some_and(|session| {
                    session.kind == PaneAgentSessionKind::Path
                        && session.value == agent_session_path.to_string_lossy().as_ref()
                })
        });
        if handoff_event && complete {
            break;
        }
        assert!(
            Instant::now() < handoff_deadline,
            "SSH agent hook handoff/metadata 未进入 Workspace: {:?}",
            workspace.pane_agent(active)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(workspace
        .take_attention_signals(active)
        .iter()
        .any(|signal| {
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
                "pane_id": pane,
                "source": source,
                "agent": "pi",
                "state": "blocked",
                "message": "approve SSH command",
                "seq": 3,
                "agent_session_path": agent_session_path,
            }),
        )
        .expect("SSH forward 报告 blocked");
    let blocked_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let _ = workspace.refresh();
        if workspace.pane_agent(active).map(|agent| agent.status) == Some(PaneAgentStatus::Blocked)
        {
            break;
        }
        assert!(
            Instant::now() < blocked_deadline,
            "SSH agent blocked 未进入 Workspace: {:?}",
            workspace.pane_agent(active)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(workspace
        .take_attention_signals(active)
        .iter()
        .any(|signal| {
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
            "pane.clear_agent_authority",
            serde_json::json!({
                "pane_id": pane,
                "source": source,
                "seq": 4,
            }),
        )
        .expect("SSH forward 清除 agent authority");
    agent_command.mark_done();
    agent_command.stop();
    let release_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let _ = workspace.refresh();
        if workspace.pane_agent(active).is_none() {
            break;
        }
        assert!(
            Instant::now() < release_deadline,
            "SSH agent 退出后未清除 Workspace agent: {:?}",
            workspace.pane_agent(active)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(workspace
        .take_attention_signals(active)
        .iter()
        .any(|signal| matches!(signal, AttentionSignal::ClearAuthoritativeStatus)));

    rt.block_on(workspace.shutdown())
        .expect("SSH Herdr shutdown 应成功");
    assert!(!local_socket.exists(), "shutdown 应清理 API forward socket");
    assert!(
        !client_socket.exists(),
        "shutdown 应清理 client forward socket"
    );
}

/// C7：SSH Host 名叫 `local`（连 loopback）时，Catalog 必须能列出隔离 tmux，
/// 且 `runtime_list` 仍是插件表。Host `local` ≠ Transport `"local"`。
#[test]
fn catalog_ssh_host_named_local_lists_isolated_tmux_and_runtime_list() {
    let _env_lock = lock_process_env();
    if !loopback_sshd_available() {
        eprintln!("skip: 无 sshd 二进制，无法自启 loopback sshd");
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux 二进制");
        return;
    }
    let sshd = LoopbackSshd::start_with_alias("cat-local", "local").expect("启动 Host local sshd");
    sshd.apply_ssh_config_env();
    assert_eq!(sshd.alias, "local");

    let socket = unique_socket("cat-local-tmux");
    create_session(&socket, "mux-ssh-local", 80, 24);
    let _tmux_guard = TmuxGuard {
        socket: socket.clone(),
    };
    std::env::set_var("MUXTERM_TEST_REMOTE_TMUX_SOCKET", &socket);
    struct EnvGuard;
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("MUXTERM_TEST_REMOTE_TMUX_SOCKET");
            std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
        }
    }
    let _env = EnvGuard;

    let mut cat = Catalog::with_builtins();
    let runtimes: Vec<String> = cat.runtime_list().into_iter().map(|r| r.id).collect();
    assert_eq!(
        runtimes,
        vec!["tmux".to_string(), "herdr".into(), "shell".into()],
        "runtime_list 是插件表，不是 SSH host / Transport id: {runtimes:?}"
    );

    let ssh_targets = cat.discover_targets("ssh").expect("ssh targets");
    assert!(
        ssh_targets.iter().any(|t| t.id == "local"),
        "discover_targets(ssh) 必须含 Host alias local: {ssh_targets:?}"
    );

    let local_targets = cat.discover_targets("local").expect("local targets");
    assert_eq!(local_targets.len(), 1, "{local_targets:?}");
    assert_eq!(
        local_targets[0].id, "",
        "Local Transport 单例 target id 是空串，不是 Host local: {local_targets:?}"
    );

    let t0 = Instant::now();
    let sessions = cat
        .discover_sessions("ssh", "local")
        .expect("ssh Host local 列出不应 Err");
    let elapsed = t0.elapsed();
    assert!(
        elapsed < Duration::from_secs(4),
        "Host local 列出必须用 2s discovery 超时，实际 {elapsed:?}"
    );
    assert!(
        sessions.iter().any(|s| {
            s.runtime_id == "tmux"
                && s.transport_id == "ssh"
                && s.target == "local"
                && s.name == "mux-ssh-local"
        }),
        "discover_sessions(ssh, local) 必须看到隔离 session mux-ssh-local: {sessions:?}"
    );

    let local_sessions = cat
        .discover_sessions("local", "")
        .expect("local transport 列出不应 Err");
    assert!(
        local_sessions.iter().all(|s| s.transport_id == "local"),
        "discover_sessions(local, \"\") 禁止串成 SSH Host local: {local_sessions:?}"
    );
}

/// C9：同一隔离 tmux 经 local 和 SSH Host `self` 必须两行。不测 archmini/cd。
#[test]
fn catalog_all_lists_local_and_ssh_self_duplicates() {
    let _env_lock = lock_process_env();
    if !loopback_sshd_available() {
        eprintln!("skip: 无 sshd 二进制，无法自启 loopback sshd");
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux 二进制");
        return;
    }
    let sshd = LoopbackSshd::start_with_alias("cat-self", "self").expect("启动 Host self sshd");
    sshd.apply_ssh_config_env();
    assert_eq!(sshd.alias, "self");

    let socket = unique_socket("cat-self-tmux");
    create_session(&socket, "mux-dup", 80, 24);
    let _tmux_guard = TmuxGuard {
        socket: socket.clone(),
    };
    std::env::set_var("MUXTERM_TEST_LOCAL_TMUX_SOCKET", &socket);
    std::env::set_var("MUXTERM_TEST_REMOTE_TMUX_SOCKET", &socket);
    std::env::set_var(
        "HERDR_SOCKET_PATH",
        format!("/tmp/muxterm-no-herdr-{}", std::process::id()),
    );
    struct EnvGuard;
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("MUXTERM_TEST_LOCAL_TMUX_SOCKET");
            std::env::remove_var("MUXTERM_TEST_REMOTE_TMUX_SOCKET");
            std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
            std::env::remove_var("HERDR_SOCKET_PATH");
        }
    }
    let _env = EnvGuard;

    let mut cat = Catalog::with_builtins();
    let sessions = cat
        .discover_sessions("all", "")
        .expect("discover_sessions(all) 不应 Err");
    assert!(
        sessions.iter().any(|s| {
            s.runtime_id == "tmux" && s.transport_id == "local" && s.name == "mux-dup"
        }),
        "all 必须含 local 的 mux-dup: {sessions:?}"
    );
    assert!(
        sessions.iter().any(|s| {
            s.runtime_id == "tmux"
                && s.transport_id == "ssh"
                && s.target == "self"
                && s.name == "mux-dup"
        }),
        "all 必须含 ssh-self 的 mux-dup（双份）: {sessions:?}"
    );
}

/// 隔离 tmux 清理（只杀自己的 -L server）。
struct TmuxGuard {
    socket: String,
}

impl Drop for TmuxGuard {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}
