//! tmux v1 命令执行器：把 TmuxV1Command 映射到 Runtime + Backend，输出 JSON envelope。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §8。
//! 所有命令输出统一 envelope：`{"version":1,"ok":true|false,...}`。

use std::time::{Duration, Instant};

use anyhow::Context;

use crate::cli::tmux_v1::{
    parse_tmux_v1, CliEnvelope, PaneCmd, SessionCmd, SplitDirection, TabCmd, Target, TmuxV1Command,
};
use crate::core::backend::TmuxBackend;
use crate::core::model::task::Task;
use crate::core::model::TerminalModel;
use crate::core::types::{PaneId, TabId, WindowId};

/// tmux v1 命令执行超时（硬限制）。
const EXEC_TIMEOUT: Duration = Duration::from_secs(15);

/// tmux 启动后等待事件就绪的轮询时间。
const READY_POLL_DURATION: Duration = Duration::from_millis(800);

/// `muxterm tmux ...` 入口：解析 + 执行 + 输出 envelope。
pub fn run_tmux_v1(args: &[String]) -> anyhow::Result<()> {
    let cmd = match parse_tmux_v1(args) {
        Ok(c) => c,
        Err(e) => {
            let env = CliEnvelope::error("PARSE_ERROR", &e);
            println!("{}", serde_json::to_string(&env).unwrap());
            return Ok(());
        }
    };

    let result = execute_tmux_v1(&cmd);
    let envelope = match result {
        Ok(data) => CliEnvelope::ok(data),
        Err(e) => CliEnvelope::error("EXEC_ERROR", &e.to_string()),
    };
    println!("{}", serde_json::to_string(&envelope).unwrap());
    Ok(())
}

/// 执行 tmux v1 命令，返回 JSON data 或错误。
fn execute_tmux_v1(cmd: &TmuxV1Command) -> anyhow::Result<serde_json::Value> {
    let deadline = Instant::now() + EXEC_TIMEOUT;
    match cmd {
        TmuxV1Command::Session(s) => execute_session(s, deadline),
        TmuxV1Command::Tab(t) => execute_tab(t, deadline),
        TmuxV1Command::Pane(p) => execute_pane(p, deadline),
    }
}

/// 构造本地 tmux RuntimeMode + TerminalModel 并连接。
fn connect_local_tmux(socket: Option<&str>, session_name: &str) -> anyhow::Result<TerminalModel> {
    // 检查 session 是否已存在
    let existing = find_existing_tmux_session(socket);
    let backend: Box<dyn crate::core::model::Backend> = if existing.as_deref() == Some(session_name)
    {
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
    Ok(model)
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

fn execute_session(cmd: &SessionCmd, deadline: Instant) -> anyhow::Result<serde_json::Value> {
    match cmd {
        SessionCmd::List { target } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let sessions = crate::core::discovery::list_local_tmux_sessions(None);
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
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote session list 尚未实现（alias={alias}）"
                )),
            }
        }
        SessionCmd::New {
            target,
            name,
            cwd: _,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let mut model = connect_local_tmux(None, name)?;
                    // 新建后等 tab 就绪
                    wait_ready(&mut model, READY_POLL_DURATION);
                    let session_name = model
                        .state()
                        .active_session()
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| name.clone());
                    Ok(serde_json::json!({
                        "session": session_name,
                        "created": true,
                    }))
                }
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote session new 尚未实现（alias={alias}）"
                )),
            }
        }
        SessionCmd::Attach { target, name } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let mut model = connect_local_tmux(None, name)?;
                    wait_ready(&mut model, READY_POLL_DURATION);
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
                }
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote session attach 尚未实现（alias={alias}）"
                )),
            }
        }
    }
}

fn execute_tab(cmd: &TabCmd, deadline: Instant) -> anyhow::Result<serde_json::Value> {
    match cmd {
        TabCmd::List { target, session } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let mut model = connect_local_tmux(None, session)?;
                    wait_ready(&mut model, READY_POLL_DURATION);
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
                }
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote tab list 尚未实现（alias={alias}）"
                )),
            }
        }
        TabCmd::New {
            target,
            session,
            name,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let mut model = connect_local_tmux(None, session)?;
                    wait_ready(&mut model, READY_POLL_DURATION);
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
                    let _ = model.poll_events();
                    wait_ready(&mut model, Duration::from_millis(500));
                    let _ = model.refresh();
                    // 找最新 tab（clone data 避免引用问题）
                    let new_tab = {
                        let state = model.state();
                        let win_id = state.active_window().map(|w| w.id);
                        win_id
                            .and_then(|wid| state.tabs(&wid).last().copied())
                            .map(|t| serde_json::json!({"id": t.id.0, "name": t.name, "active": t.active}))
                            .unwrap_or(serde_json::json!({}))
                    };
                    Ok(new_tab)
                }
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
            session,
            tab,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let mut model = connect_local_tmux(None, session)?;
                    wait_ready(&mut model, READY_POLL_DURATION);
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
                }
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote pane list 尚未实现（alias={alias}）"
                )),
            }
        }
        PaneCmd::Split {
            target,
            session,
            pane,
            direction,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let mut model = connect_local_tmux(None, session)?;
                    wait_ready(&mut model, READY_POLL_DURATION);
                    let _ = model.refresh();
                    let dir = match direction {
                        SplitDirection::Horizontal => {
                            crate::core::model::layout::SplitDir::Horizontal
                        }
                        SplitDirection::Vertical => crate::core::model::layout::SplitDir::Vertical,
                    };
                    model.execute(Task::SplitPane {
                        target: Some(PaneId(*pane)),
                        dir,
                        command: None,
                        workdir: None,
                    })?;
                    let _ = model.poll_events();
                    wait_ready(&mut model, Duration::from_millis(500));
                    let _ = model.refresh();
                    // 找最新 pane（clone data 避免引用问题）
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
                }
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote pane split 尚未实现（alias={alias}）"
                )),
            }
        }
        PaneCmd::SendKeys {
            target,
            session,
            pane,
            text,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let mut model = connect_local_tmux(None, session)?;
                    wait_ready(&mut model, READY_POLL_DURATION);
                    let _ = model.refresh();
                    use crate::core::terminal::input::KeyEvent;
                    let keys: Vec<KeyEvent> = text.chars().map(KeyEvent::Char).collect();
                    model.execute(Task::SendKeys {
                        target: PaneId(*pane),
                        keys,
                    })?;
                    let _ = model.poll_events();
                    Ok(serde_json::json!({"sent": true, "pane": pane}))
                }
                Target::Ssh { alias } => Err(anyhow::anyhow!(
                    "SSH remote send-keys 尚未实现（alias={alias}）"
                )),
            }
        }
        PaneCmd::Capture {
            target,
            session,
            pane,
            lines,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let mut model = connect_local_tmux(None, session)?;
                    wait_ready(&mut model, READY_POLL_DURATION);
                    let _ = model.refresh();
                    let output = model.state().pane_output(&PaneId(*pane)).unwrap_or(&[]);
                    let text = String::from_utf8_lossy(output);
                    let captured = if let Some(n) = lines {
                        let all_lines: Vec<&str> = text.lines().collect();
                        let start = all_lines.len().saturating_sub(*n);
                        all_lines[start..].join("\n")
                    } else {
                        text.to_string()
                    };
                    Ok(serde_json::json!({
                        "pane": pane,
                        "output": captured,
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
