//! tmux CLI 命令执行器：把 TmuxCliCommand 映射到 Runtime，输出 JSON envelope。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §8。
//! 所有命令输出统一 envelope：`{"ok":true|false,...}`。

use std::time::{Duration, Instant};

use anyhow::Context;

use crate::platform::cli::tmux_cli::{
    parse_tmux_cli, CliEnvelope, PaneCmd, SessionCmd, SplitDirection, TabCmd, Target,
    TmuxCliCommand,
};
use crate::platform::ffi_client::{ClientTask, FfiClient};

/// tmux CLI 命令执行超时（硬限制）。
const EXEC_TIMEOUT: Duration = Duration::from_secs(15);

/// tmux 启动后等待事件就绪的轮询时间。
const READY_POLL_DURATION: Duration = Duration::from_millis(800);

/// 显式 snapshot 后的最短等待时间。
///
/// pause/capture 的权威响应可能晚于第一条 live tail；过早返回会把 runtime
/// 提前关闭，从而丢掉完整快照。
const SNAPSHOT_MIN_WAIT: Duration = Duration::from_secs(1);

/// A short-lived CLI client owns one Core FFI handle for the whole command.
///
/// The handle owns the Runtime and its Tokio runtime; the CLI only queries
/// owned workspace DTOs and submits frontend tasks through `FfiClient`.
struct FfiTmuxClient {
    client: FfiClient,
    workspace_id: String,
}

impl FfiTmuxClient {
    fn connect(socket: Option<&str>, session_name: &str) -> anyhow::Result<Self> {
        let exists = FfiClient::discover_tmux_sessions("local", None, socket)
            .with_context(|| "tmux session discovery failed")?
            .iter()
            .any(|session| session.name == session_name);
        if !exists {
            FfiClient::create_workspace("tmux", None, socket, session_name, "")
                .with_context(|| format!("create tmux session failed: {session_name}"))?;
        }

        let client = FfiClient::new_connect("tmux", socket, Some(session_name), None, None)
            .with_context(|| format!("attach tmux session failed: {session_name}"))?;
        let workspaces = client.workspace_list()?;
        let workspace_id = workspaces
            .iter()
            .find(|workspace| workspace.active)
            .or_else(|| workspaces.first())
            .map(|workspace| workspace.id.clone())
            .ok_or_else(|| anyhow::anyhow!("Core returned no tmux workspace"))?;
        Ok(Self {
            client,
            workspace_id,
        })
    }

    fn poll(&self) {
        let _ = self.client.poll_workspace_events();
    }

    fn tabs(&self) -> Vec<crate::platform::ffi_client::ClientTab> {
        self.client.get_workspace_tabs(&self.workspace_id)
    }

    fn panes(&self, tab_id: u32) -> Vec<crate::platform::ffi_client::ClientPane> {
        self.client.get_workspace_panes(&self.workspace_id, tab_id)
    }

    fn active_tab(&self) -> Option<crate::platform::ffi_client::ClientTab> {
        self.tabs().into_iter().find(|tab| tab.is_active)
    }

    fn pane_output(&self, pane_id: u32) -> Vec<u8> {
        self.client
            .get_workspace_pane_output(&self.workspace_id, pane_id)
    }

    fn execute(&self, task: ClientTask) -> anyhow::Result<()> {
        let rc = self.client.execute_workspace_task(&self.workspace_id, task);
        if rc == 0 {
            Ok(())
        } else {
            anyhow::bail!("Core FFI task failed with code {rc}")
        }
    }

    fn send_input(&self, pane_id: u32, data: &[u8]) -> anyhow::Result<()> {
        let rc = self
            .client
            .send_workspace_input(&self.workspace_id, pane_id, data);
        if rc == 0 {
            Ok(())
        } else {
            anyhow::bail!("Core FFI input failed with code {rc}")
        }
    }

    fn shutdown(&self) {
        let _ = self.client.shutdown();
    }
}

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

/// 通过 FFI client 构造本地 tmux workspace，在 live handle 内执行 fn。
fn with_local_tmux<F>(
    socket: Option<&str>,
    session_name: &str,
    _deadline: Instant,
    f: F,
) -> anyhow::Result<serde_json::Value>
where
    F: FnOnce(&mut FfiTmuxClient) -> anyhow::Result<serde_json::Value>,
{
    let mut client = FfiTmuxClient::connect(socket, session_name)?;
    wait_ready(&mut client, READY_POLL_DURATION);

    // 在 runtime 存活期间执行命令
    let result = f(&mut client);

    // 命令执行后，短暂等待事件回流（最多 500ms）
    wait_events_brief(&mut client);

    // 优雅关闭
    client.shutdown();
    result
}

/// 轮询事件直到有 tab 或超时。
fn wait_ready(client: &mut FfiTmuxClient, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        client.poll();
        if client.active_tab().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 等待显式请求的权威 pane 快照。
///
/// live tail 可能先于 capture 响应到达。若在第一条变化输出后就返回，runtime
/// 会被提前关闭，完整快照来不及替换 live tail。
fn wait_cli_pane_capture(client: &mut FfiTmuxClient, pane_id: u32, deadline: Instant) -> String {
    let requested_at = Instant::now();
    let mut previous = client.pane_output(pane_id);
    let mut previous_at = Instant::now();

    while Instant::now() < deadline {
        client.poll();
        let current = client.pane_output(pane_id);
        if current != previous {
            previous = current;
            previous_at = Instant::now();
        }

        let waited_for_snapshot = requested_at.elapsed() >= SNAPSHOT_MIN_WAIT;
        let stable = previous_at.elapsed() >= Duration::from_millis(100);
        if waited_for_snapshot && stable && !previous.is_empty() {
            return String::from_utf8_lossy(&previous).to_string();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    String::from_utf8_lossy(&previous).to_string()
}

/// 截取 capture 输出的末尾 N 行，同时忽略屏幕网格末尾的空白行。
///
/// 权威 snapshot 会保留 pane 的完整网格（包括尾部空行）；`--lines` 的 CLI
/// 语义应该返回最后 N 行有意义的输出，而不是让空行吞掉最近命令。
fn is_blank_or_control_line(line: &str) -> bool {
    let mut escaped = false;
    let mut in_csi = false;

    for ch in line.chars() {
        if in_csi {
            if ('\u{40}'..='\u{7e}').contains(&ch) {
                in_csi = false;
            }
            continue;
        }
        if escaped {
            escaped = false;
            if ch == '[' {
                in_csi = true;
            }
            continue;
        }
        if ch == '\u{1b}' {
            escaped = true;
            continue;
        }
        if ch.is_control() || ch.is_whitespace() {
            continue;
        }
        return false;
    }
    true
}

fn truncate_capture_lines(text: String, lines: Option<usize>) -> String {
    let Some(n) = lines else {
        return text;
    };
    let mut all_lines: Vec<&str> = text.trim_end().lines().collect();

    // 先取掉末尾的控制/空行；其中非空白行是 cursor/mode 等 VT 状态，
    // 需要保留。空白行只是 pane 网格 padding，不应占用 `--lines` 名额。
    let mut state_suffix = Vec::new();
    while all_lines
        .last()
        .is_some_and(|line| is_blank_or_control_line(line))
    {
        let line = all_lines.pop().expect("all_lines is non-empty");
        if !line.trim().is_empty() {
            state_suffix.push(line);
        }
    }
    while all_lines.last().is_some_and(|line| line.trim().is_empty()) {
        all_lines.pop();
    }

    let start = all_lines.len().saturating_sub(n);
    let selected = all_lines[start..].to_vec();
    let mut output = selected.join("\n");
    if let Some(first_suffix) = state_suffix.last() {
        output.push_str(first_suffix);
    }
    for suffix in state_suffix.iter().rev().skip(1) {
        output.push('\n');
        output.push_str(suffix);
    }
    output
}

/// 等待 pane 数量变化（确认 tmux 已处理 split/new-window 等命令）。
///
/// 在 deadline 内持续 refresh+poll，当 pane 数 ≥ `min_panes` 时立即返回。
/// 如果超时仍未达到，也返回（调用方通过后续查询断言结果）。
fn wait_for_pane_count(client: &mut FfiTmuxClient, min_panes: usize, deadline: Instant) {
    while Instant::now() < deadline {
        client.poll();
        if let Some(tab) = client.active_tab() {
            if client.panes(tab.id).len() >= min_panes {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 等待 tab 数量变化。
fn wait_for_tab_count(client: &mut FfiTmuxClient, min_tabs: usize, deadline: Instant) {
    while Instant::now() < deadline {
        client.poll();
        if client.tabs().len() >= min_tabs {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 等待事件回流（给 tmux 一小段处理时间，最多 500ms）。
fn wait_events_brief(client: &mut FfiTmuxClient) {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        client.poll();
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn execute_session(cmd: &SessionCmd, deadline: Instant) -> anyhow::Result<serde_json::Value> {
    match cmd {
        SessionCmd::List { target, socket } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    let sessions =
                        FfiClient::discover_tmux_sessions("local", None, socket.as_deref())?;
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
                    Ok(serde_json::json!({"workspaces": arr}))
                }
                Target::Ssh { alias } => {
                    let sessions =
                        FfiClient::discover_tmux_sessions("ssh", Some(alias), socket.as_deref())?;
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
                    Ok(serde_json::json!({"workspaces": arr}))
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
                Target::Local => with_local_tmux(socket.as_deref(), name, deadline, |client| {
                    wait_ready(client, READY_POLL_DURATION);
                    Ok(serde_json::json!({
                        "session": name,
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
                Target::Local => with_local_tmux(socket.as_deref(), name, deadline, |client| {
                    wait_ready(client, READY_POLL_DURATION);
                    // 无 GUI 的 CLI 不发送 ResizeClient；显式请求权威快照，
                    // 避免 attach 首屏被 UI 尺寸门闩无限推迟。
                    let pane_id = client
                        .active_tab()
                        .and_then(|tab| {
                            client.panes(tab.id).into_iter().find(|pane| pane.is_active)
                        })
                        .map(|pane| pane.id);
                    if let Some(pane_id) = pane_id {
                        let _ = client.execute(ClientTask::RequestPaneSnapshot { pane_id });
                        let _ = wait_cli_pane_capture(client, pane_id, deadline);
                    }
                    let tabs = client.tabs().len() as u32;
                    let panes: Vec<serde_json::Value> = client
                        .active_tab()
                        .map(|tab| {
                            client
                                .panes(tab.id)
                                .iter()
                                .map(|pane| {
                                    let output =
                                        String::from_utf8_lossy(&client.pane_output(pane.id))
                                            .to_string();
                                    serde_json::json!({
                                        "id": pane.id,
                                        "output": output,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Ok(serde_json::json!({
                        "session": name,
                        "attached": true,
                        "tabs": tabs,
                        "panes": panes,
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
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |client| {
                    wait_ready(client, READY_POLL_DURATION);
                    client.poll();
                    let tabs: Vec<serde_json::Value> = client
                        .tabs()
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "id": t.id,
                                "name": t.name,
                                "active": t.is_active,
                            })
                        })
                        .collect();
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
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |client| {
                    wait_ready(client, READY_POLL_DURATION);
                    client.poll();
                    let rc = client
                        .client
                        .new_workspace_tab(&client.workspace_id, name.as_deref());
                    if rc != 0 {
                        anyhow::bail!("Core FFI new tab failed with code {rc}");
                    }
                    // 等待 tab 数增加（确认 tmux 已处理 new-window，最多 2s）
                    wait_for_tab_count(client, 2, Instant::now() + Duration::from_secs(2));
                    client.poll();
                    let new_tab = client
                        .tabs()
                        .last()
                        .map(|t| {
                            serde_json::json!({
                                "id": t.id,
                                "name": t.name,
                                "active": t.is_active
                            })
                        })
                        .unwrap_or(serde_json::json!({}));
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
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |client| {
                    wait_ready(client, READY_POLL_DURATION);
                    client.poll();
                    let tab_id = tab.or_else(|| client.active_tab().map(|t| t.id));
                    let panes: Vec<serde_json::Value> = if let Some(tid) = tab_id {
                        client
                            .panes(tid)
                            .iter()
                            .map(|p| {
                                serde_json::json!({
                                    "id": p.id,
                                    "active": p.is_active,
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
                    let panes = crate::core::discovery::list_ssh_tmux_panes(
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
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |client| {
                    wait_ready(client, READY_POLL_DURATION);
                    client.poll();
                    // 使用 CLI 传入的 pane ID（muxterm pane id = tmux %N 的 N）
                    client.execute(ClientTask::SplitPane {
                        pane_id: *pane,
                        horizontal: matches!(direction, SplitDirection::Horizontal),
                    })?;
                    // 等待 pane 数增加（确认 tmux 已处理 split-window，最多 3s）
                    wait_for_pane_count(client, 2, Instant::now() + Duration::from_secs(3));
                    client.poll();
                    let new_pane = client
                        .active_tab()
                        .and_then(|tab| client.panes(tab.id).last().cloned())
                        .map(|p| {
                            serde_json::json!({
                                "id": p.id,
                                "active": p.is_active,
                                "cols": p.cols,
                                "rows": p.rows,
                            })
                        })
                        .unwrap_or(serde_json::json!({}));
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
                Target::Local => with_local_tmux(socket.as_deref(), session, deadline, |client| {
                    wait_ready(client, READY_POLL_DURATION);
                    client.poll();
                    // 发送文本 + Enter（让 shell 执行命令）
                    let mut bytes = text.as_bytes().to_vec();
                    bytes.push(b'\r');
                    client.send_input(*pane, &bytes)?;
                    // 等待 shell 执行并产生输出（最多 2s）
                    let send_deadline = Instant::now() + Duration::from_secs(2);
                    while Instant::now() < send_deadline {
                        client.poll();
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
            session,
            pane,
            lines,
        } => {
            check_timeout(deadline)?;
            match target {
                Target::Local => {
                    // 走 core runtime 的 pane 输出（platform 不再拼 tmux）。
                    with_local_tmux(socket.as_deref(), session, deadline, |client| {
                        wait_ready(client, READY_POLL_DURATION);
                        // CLI 可以读取后台 tab 的 pane；显式请求权威 Surface，
                        // 不依赖 attach 时活动 tab 的首屏 seed。
                        let _ = client.execute(ClientTask::RequestPaneSnapshot { pane_id: *pane });
                        let text = wait_cli_pane_capture(client, *pane, deadline);
                        let text = truncate_capture_lines(text, *lines);
                        Ok(serde_json::json!({
                            "pane": pane,
                            "output": text,
                        }))
                    })
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

#[cfg(test)]
mod tests {
    use super::truncate_capture_lines;

    #[test]
    fn capture_lines_ignores_trailing_blank_grid_lines() {
        let text = "marker\nprompt\n\n\n\n\n\n\n\n\n\n\n\n";

        assert_eq!(
            truncate_capture_lines(text.to_string(), Some(10)),
            "marker\nprompt"
        );
    }

    #[test]
    fn capture_lines_preserves_terminal_state_suffix() {
        let text = "marker\nprompt\n\n\n\n\n\n\n\n\n\u{1b}[9;3H\u{1b}[?25h";

        assert_eq!(
            truncate_capture_lines(text.to_string(), Some(10)),
            "marker\nprompt\u{1b}[9;3H\u{1b}[?25h"
        );
    }

    #[test]
    fn capture_without_line_limit_preserves_output() {
        let text = "marker\n\n";

        assert_eq!(truncate_capture_lines(text.to_string(), None), "marker\n\n");
    }
}
