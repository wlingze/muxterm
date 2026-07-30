//! tmux CLI 命令执行器：把 TmuxCliCommand 映射到 Runtime + Backend，输出 JSON envelope。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §8。
//! 所有命令输出统一 envelope：`{"ok":true|false,...}`。

use std::time::{Duration, Instant};

use anyhow::Context;

use crate::platform::cli::tmux_cli::{
    parse_tmux_cli, CliEnvelope, PaneCmd, SessionCmd, SplitDirection, TabCmd, Target,
    TmuxCliCommand,
};
use crate::protocol::model::task::Task;
use crate::protocol::model::TerminalModel;
use crate::runtime::tmux::TmuxBackend;
use crate::types::{PaneId, TabId, WindowId};

/// tmux CLI 命令执行超时（硬限制）。
const EXEC_TIMEOUT: Duration = Duration::from_secs(15);

/// tmux 启动后等待事件就绪的轮询时间。
const READY_POLL_DURATION: Duration = Duration::from_millis(800);

/// `muxterm tmux ...` 入口：解析 + 执行 + 输出 envelope。
pub fn run_tmux_cli(args: &[String]) -> anyhow::Result<()> {
    let cmd = match parse_tmux_cli(args) {
        Ok(c) => c,
        Err(e) => {
            let env = CliEnvelope::error("PARSE_ERROR", &e);
            println!("{}", serde_json::to_string(&env).unwrap());
            return Ok(());
        }
    };

    let result = execute_tmux_cli(&cmd);
    let envelope = match result {
        Ok(data) => CliEnvelope::ok(data),
        Err(e) => CliEnvelope::error("EXEC_ERROR", &e.to_string()),
    };
    println!("{}", serde_json::to_string(&envelope).unwrap());
    Ok(())
}

/// 执行 tmux CLI 命令，返回 JSON data 或错误。
fn execute_tmux_cli(cmd: &TmuxCliCommand) -> anyhow::Result<serde_json::Value> {
    let deadline = Instant::now() + EXEC_TIMEOUT;
    match cmd {
        TmuxCliCommand::Session(s) => execute_session(s, deadline),
        TmuxCliCommand::Tab(t) => execute_tab(t, deadline),
        TmuxCliCommand::Pane(p) => execute_pane(p, deadline),
    }
}

/// 构造本地 tmux backend + TerminalModel，在 runtime 内执行 fn 并返回结果。
///
/// **关键**：runtime 必须在整个命令生命周期内存活，否则 sender task 被杀，
/// 命令无法到达 tmux。
fn with_local_tmux<F>(
    socket: Option<&str>,
    session_name: &str,
    _deadline: Instant,
    f: F,
) -> anyhow::Result<serde_json::Value>
where
    F: FnOnce(&mut TerminalModel) -> anyhow::Result<serde_json::Value>,
{
    let session_exists = tmux_session_exists(socket, session_name);
    let backend: Box<dyn crate::protocol::model::Backend> = if session_exists {
        Box::new(TmuxBackend::new_with_attach(socket, session_name))
    } else {
        Box::new(TmuxBackend::new_with_session_name(socket, session_name))
    };
    let mut model = TerminalModel::new(backend);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()?;
    rt.block_on(model.connect()).context("tmux connect 失败")?;
    let _ = model.poll_events();
    wait_ready(&mut model, READY_POLL_DURATION);

    // 在 runtime 存活期间执行命令
    let result = f(&mut model)?;

    // 命令执行后，短暂等待事件回流（最多 500ms）
    wait_events_brief(&mut model);

    // 优雅关闭
    let _ = rt.block_on(model.shutdown());
    Ok(result)
}

/// 轮询事件直到有 tab 或超时。
fn wait_ready(model: &mut TerminalModel, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        let _ = model.refresh();
        if model.state().active_tab().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 等待 pane 数量变化（确认 tmux 已处理 split/new-window 等命令）。
///
/// 在 deadline 内持续 refresh+poll，当 pane 数 ≥ `min_panes` 时立即返回。
/// 如果超时仍未达到，也返回（调用方通过后续查询断言结果）。
fn wait_for_pane_count(model: &mut TerminalModel, min_panes: usize, deadline: Instant) {
    while Instant::now() < deadline {
        let _ = model.refresh();
        if let Some(tab) = model.state().active_tab() {
            if model.state().panes(&tab.id).len() >= min_panes {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 等待 tab 数量变化。
fn wait_for_tab_count(model: &mut TerminalModel, min_tabs: usize, deadline: Instant) {
    while Instant::now() < deadline {
        let _ = model.refresh();
        if let Some(win) = model.state().active_window() {
            if model.state().tabs(&win.id).len() >= min_tabs {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 等待事件回流（给 tmux 一小段处理时间，最多 500ms）。
fn wait_events_brief(model: &mut TerminalModel) {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let _ = model.refresh();
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn find_existing_tmux_session(socket: Option<&str>) -> Option<String> {
    let mut cmd = std::process::Command::new("tmux");
    if let Some(s) = socket {
        cmd.args(["-L", s]);
    }
    cmd.args(["list-sessions", "-F", "#{session_name}"]);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let output = cmd.output().ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(|s| s.trim().to_string())
}

/// 检查指定名称的 tmux session 是否存在。
fn tmux_session_exists(socket: Option<&str>, name: &str) -> bool {
    let mut cmd = std::process::Command::new("tmux");
    if let Some(s) = socket {
        cmd.args(["-L", s]);
    }
    cmd.args(["has-session", "-t", name]);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

fn execute_session(cmd: &SessionCmd, deadline: Instant) -> anyhow::Result<serde_json::Value> {
    match cmd {
        SessionCmd::List { target, socket } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let sessions = crate::discovery::list_local_tmux_sessions(socket.as_deref());
                    let arr: Vec<serde_json::Value> = sessions
                        .iter()
                        .map(|s| {
                            serde_json::json!({
                                "name": s.name,
                                "windows": s.windows,
                                "attached": s.attached,
                                "created": s.created,
                            })
                        })
                        .collect();
                    Ok(serde_json::json!({"sessions": arr}))
                }
                Target::Ssh { alias } => {
                    let ssh_config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
                    let sessions = crate::discovery::list_ssh_tmux_sessions(
                        alias,
                        ssh_config.as_deref(),
                        socket.as_deref(),
                        std::time::Duration::from_secs(10),
                    )?;
                    let arr: Vec<serde_json::Value> = sessions
                        .iter()
                        .map(|s| {
                            serde_json::json!({
                                "name": s.name,
                                "windows": s.windows,
                                "attached": s.attached,
                                "created": s.created,
                            })
                        })
                        .collect();
                    Ok(serde_json::json!({"sessions": arr}))
                }
            }
        }
        SessionCmd::New {
            target,
            socket,
            name,
            cwd: _,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => with_local_tmux(socket.as_deref(), name, deadline, |model| {
                    wait_ready(model, READY_POLL_DURATION);
                    let session_name = model
                        .state()
                        .active_session()
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| name.clone());
                    Ok(serde_json::json!({
                        "session": session_name,
                        "created": true,
                    }))
                }),
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote session new 尚未实现（alias={alias}）"
                )),
            }
        }
        SessionCmd::Attach {
            target,
            socket,
            name,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => with_local_tmux(socket.as_deref(), name, deadline, |model| {
                    wait_ready(model, READY_POLL_DURATION);
                    let tabs = model
                        .state()
                        .active_window()
                        .map(|w| model.state().tabs(&w.id).len() as u32)
                        .unwrap_or(0);
                    Ok(serde_json::json!({
                        "session": name,
                        "attached": true,
                        "tabs": tabs,
                    }))
                }),
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote session attach 尚未实现（alias={alias}）"
                )),
            }
        }
    }
}

fn execute_tab(cmd: &TabCmd, deadline: Instant) -> anyhow::Result<serde_json::Value> {
    match cmd {
        TabCmd::List {
            target,
            socket,
            session,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |model| {
                    wait_ready(model, READY_POLL_DURATION);
                    let _ = model.refresh();
                    let tabs: Vec<serde_json::Value> = model
                        .state()
                        .active_window()
                        .map(|w| {
                            model
                                .state()
                                .tabs(&w.id)
                                .iter()
                                .map(|t| {
                                    serde_json::json!({
                                        "id": t.id.0,
                                        "name": t.name,
                                        "active": t.active,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Ok(serde_json::json!({"tabs": tabs}))
                }),
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote tab list 尚未实现（alias={alias}）"
                )),
            }
        }
        TabCmd::New {
            target,
            socket,
            session,
            name,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |model| {
                    wait_ready(model, READY_POLL_DURATION);
                    let _ = model.refresh();
                    let wid = model
                        .state()
                        .active_window()
                        .map(|w| w.id)
                        .unwrap_or(WindowId(1));
                    model.execute(Task::NewTab {
                        window: wid,
                        name: name.clone(),
                        command: None,
                        workdir: None,
                    })?;
                    // 等待 tab 数增加（确认 tmux 已处理 new-window，最多 2s）
                    wait_for_tab_count(model, 2, Instant::now() + Duration::from_secs(2));
                    let _ = model.refresh();
                    let new_tab = {
                        let state = model.state();
                        let win_id = state.active_window().map(|w| w.id);
                        win_id
                            .and_then(|wid| state.tabs(&wid).last().copied())
                            .map(|t| {
                                serde_json::json!({
                                    "id": t.id.0,
                                    "name": t.name,
                                    "active": t.active
                                })
                            })
                            .unwrap_or(serde_json::json!({}))
                    };
                    Ok(new_tab)
                }),
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote tab new 尚未实现（alias={alias}）"
                )),
            }
        }
    }
}

fn execute_pane(cmd: &PaneCmd, deadline: Instant) -> anyhow::Result<serde_json::Value> {
    match cmd {
        PaneCmd::List {
            target,
            socket,
            session,
            tab,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |model| {
                    wait_ready(model, READY_POLL_DURATION);
                    let _ = model.refresh();
                    let tab_id = tab
                        .map(TabId)
                        .or_else(|| model.state().active_tab().map(|t| t.id));
                    let panes: Vec<serde_json::Value> = if let Some(tid) = tab_id {
                        model
                            .state()
                            .panes(&tid)
                            .iter()
                            .map(|p| {
                                serde_json::json!({
                                    "id": p.id.0,
                                    "active": p.active,
                                    "cols": p.cols,
                                    "rows": p.rows,
                                    "title": p.title,
                                })
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    Ok(serde_json::json!({"panes": panes}))
                }),
                Target::Ssh { alias } => {
                    let ssh_config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
                    let panes = crate::discovery::list_ssh_tmux_panes(
                        alias,
                        ssh_config.as_deref(),
                        socket.as_deref(),
                        session,
                        std::time::Duration::from_secs(10),
                    )?;
                    let arr: Vec<serde_json::Value> = panes
                        .iter()
                        .map(|(id, active, cols, rows, title)| {
                            serde_json::json!({
                                "id": id,
                                "active": active,
                                "cols": cols,
                                "rows": rows,
                                "title": title,
                            })
                        })
                        .collect();
                    Ok(serde_json::json!({"panes": arr}))
                }
            }
        }
        PaneCmd::Split {
            target,
            socket,
            session,
            pane,
            direction,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |model| {
                    wait_ready(model, READY_POLL_DURATION);
                    let _ = model.refresh();
                    let dir = match direction {
                        SplitDirection::Horizontal => {
                            crate::protocol::model::layout::SplitDir::Horizontal
                        }
                        SplitDirection::Vertical => {
                            crate::protocol::model::layout::SplitDir::Vertical
                        }
                    };
                    // 使用 CLI 传入的 pane ID（muxterm pane id = tmux %N 的 N）
                    model.execute(Task::SplitPane {
                        target: Some(PaneId(*pane)),
                        dir,
                        command: None,
                        workdir: None,
                    })?;
                    // 等待 pane 数增加（确认 tmux 已处理 split-window，最多 3s）
                    wait_for_pane_count(model, 2, Instant::now() + Duration::from_secs(3));
                    let _ = model.refresh();
                    let new_pane = {
                        let state = model.state();
                        let tab_id = state.active_tab().map(|t| t.id);
                        tab_id
                            .and_then(|tid| state.panes(&tid).last().copied())
                            .map(|p| {
                                serde_json::json!({
                                    "id": p.id.0,
                                    "active": p.active,
                                    "cols": p.cols,
                                    "rows": p.rows,
                                })
                            })
                            .unwrap_or(serde_json::json!({}))
                    };
                    Ok(new_pane)
                }),
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote pane split 尚未实现（alias={alias}）"
                )),
            }
        }
        PaneCmd::SendKeys {
            target,
            socket,
            session,
            pane,
            text,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |model| {
                    wait_ready(model, READY_POLL_DURATION);
                    let _ = model.refresh();
                    use crate::terminal::input::KeyEvent;
                    // 发送文本 + Enter（让 shell 执行命令）
                    let mut keys: Vec<KeyEvent> = text.chars().map(KeyEvent::Char).collect();
                    keys.push(KeyEvent::Enter);
                    model.execute(Task::SendKeys {
                        target: PaneId(*pane),
                        keys,
                    })?;
                    // 等待 shell 执行并产生输出（最多 2s）
                    let send_deadline = Instant::now() + Duration::from_secs(2);
                    while Instant::now() < send_deadline {
                        let _ = model.refresh();
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Ok(serde_json::json!({"sent": true, "pane": pane}))
                }),
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote send-keys 尚未实现（alias={alias}）"
                )),
            }
        }
        PaneCmd::Capture {
            target,
            socket,
            session: _,
            pane,
            lines,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    // 使用 raw tmux capture-pane 获取可靠输出（不依赖 -CC %output 事件）
                    let mut tmux_cmd = std::process::Command::new("tmux");
                    if let Some(s) = socket {
                        tmux_cmd.args(["-L", s]);
                    }
                    tmux_cmd.args(["capture-pane", "-t", &format!("%{}", pane), "-p"]);
                    if let Some(n) = lines {
                        tmux_cmd.args(["-S", &format!("-{}", n)]);
                    }
                    let output = tmux_cmd.output().context("tmux capture-pane 失败")?;
                    let text = String::from_utf8_lossy(&output.stdout).to_string();
                    Ok(serde_json::json!({
                        "pane": pane,
                        "output": text,
                    }))
                }
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote capture 尚未实现（alias={alias}）"
                )),
            }
        }
    }
}

fn check_timeout(deadline: Instant) -> anyhow::Result<()> {
    if Instant::now() >= deadline {
        anyhow::bail!("命令执行超时（{}s）", EXEC_TIMEOUT.as_secs());
    }
    Ok(())
}
