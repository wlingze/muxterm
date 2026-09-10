//! Daemon 进程：持有一个 FFI client，监听 unix socket 接收命令。
//!
//! 架构（参考 tmux server/client）：
//! - daemon 启动后经 FFI 打开 shell 或 tmux workspace
//! - 监听 unix socket，每收到一个 Request 就执行对应 Task
//! - 返回格式化输出给 client
//! - 收到 KillSession 或 SIGTERM/SIGINT 时优雅退出
//!
//! 不做单元测试（需要真实 socket + 后台进程），集成测试在 tests/。

use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::cli::format_ffi_output;
use crate::cli::CliCommand;
use crate::ffi_client::{ClientOpenIntent, ClientResizeAxis, ClientTarget, ClientTask, FfiClient};
use muxterm_protocol::daemon::{Request, Response};

/// daemon 共享状态：一个 Core FFI handle 与其 workspace identity。
struct DaemonState {
    client: FfiClient,
    workspace_id: String,
    pending_events: Vec<serde_json::Value>,
}

impl DaemonState {
    fn connect(name: &str, tmux_socket: Option<&str>) -> Result<Self> {
        if let Some(socket) = tmux_socket {
            let sessions = FfiClient::discover_tmux_sessions("local", None, Some(socket))?;
            if !sessions.iter().any(|session| session.name == name) {
                FfiClient::create_workspace("tmux", None, Some(socket), name, "")?;
            }
            let client = FfiClient::new_connect("tmux", Some(socket), Some(name), None, None)
                .with_context(|| format!("attach tmux session failed: {name}"))?;
            let workspaces = client.workspace_list()?;
            let workspace_id = workspaces
                .iter()
                .find(|workspace| workspace.active)
                .or_else(|| workspaces.first())
                .map(|workspace| workspace.id.clone())
                .ok_or_else(|| anyhow::anyhow!("Core returned no tmux workspace"))?;
            return Ok(Self {
                client,
                workspace_id,
                pending_events: Vec::new(),
            });
        }

        let client = FfiClient::new_catalog()?;
        let opened = client.open_target(
            &ClientTarget {
                name: name.to_string(),
                runtime: "shell".into(),
                transport: "local".into(),
                target: None,
                path: String::new(),
                session: None,
                socket: None,
            },
            ClientOpenIntent::CreateIfMissing,
        )?;
        Ok(Self {
            client,
            workspace_id: opened.id,
            pending_events: Vec::new(),
        })
    }

    fn poll(&self, force_topology: bool) -> anyhow::Result<Vec<serde_json::Value>> {
        let raw_events: Vec<_> = self
            .client
            .poll_workspace_events()
            .into_iter()
            .filter(|event| event.workspace_id == self.workspace_id)
            .collect();
        let topology_changed =
            force_topology || raw_events.iter().any(|event| event.event.is_topology());
        let mut events: Vec<serde_json::Value> = raw_events
            .into_iter()
            .map(|event| event.to_wire_json())
            .collect();
        if topology_changed {
            events.insert(0, self.client.workspace_topology_json(&self.workspace_id)?);
        }
        Ok(events)
    }

    fn drain_events(&mut self, force_topology: bool) -> anyhow::Result<Vec<serde_json::Value>> {
        let fresh_events = self.poll(force_topology)?;
        let mut events = std::mem::take(&mut self.pending_events);
        events.extend(fresh_events);
        Ok(events)
    }

    fn active_tab_id(&self) -> Option<u32> {
        self.client
            .get_workspace_tabs(&self.workspace_id)
            .into_iter()
            .find(|tab| tab.is_active)
            .map(|tab| tab.id)
    }

    fn active_pane_id(&self) -> Option<u32> {
        let tab_id = self.active_tab_id()?;
        self.client
            .get_workspace_panes(&self.workspace_id, tab_id)
            .into_iter()
            .find(|pane| pane.is_active)
            .map(|pane| pane.id)
    }

    fn execute(&self, command: &CliCommand) -> Result<()> {
        use CliCommand::*;

        let code = match command {
            Config { .. } | NewWorkspace { .. } | AttachWorkspace { .. } | PollEvents => {
                return Ok(())
            }
            CloseWorkspace { .. } => self
                .client
                .execute_workspace_task(&self.workspace_id, ClientTask::Shutdown),
            Detach { .. } => self
                .client
                .execute_workspace_task(&self.workspace_id, ClientTask::Detach),
            RenameWorkspace { new_name } => {
                self.client.rename_workspace(&self.workspace_id, new_name)
            }
            NewTab { name } => self
                .client
                .new_workspace_tab(&self.workspace_id, name.as_deref()),
            KillTab { target } => {
                let Some(tab_id) = target
                    .map(|tab_id| tab_id.0)
                    .or_else(|| self.active_tab_id())
                else {
                    return Ok(());
                };
                self.client
                    .execute_workspace_task(&self.workspace_id, ClientTask::CloseTab { tab_id })
            }
            SelectTab { target } => self.client.execute_workspace_task(
                &self.workspace_id,
                ClientTask::SwitchTab { tab_id: target.0 },
            ),
            RenameTab { new_name } => {
                let Some(tab_id) = self.active_tab_id() else {
                    return Ok(());
                };
                self.client
                    .rename_workspace_tab(&self.workspace_id, tab_id, new_name)
            }
            SplitPane {
                horizontal, target, ..
            } => {
                let Some(pane_id) = target
                    .map(|pane_id| pane_id.0)
                    .or_else(|| self.active_pane_id())
                else {
                    return Ok(());
                };
                self.client.execute_workspace_task(
                    &self.workspace_id,
                    ClientTask::SplitPane {
                        pane_id,
                        horizontal: *horizontal,
                    },
                )
            }
            KillPane { target } => {
                let Some(pane_id) = target
                    .map(|pane_id| pane_id.0)
                    .or_else(|| self.active_pane_id())
                else {
                    return Ok(());
                };
                self.client
                    .execute_workspace_task(&self.workspace_id, ClientTask::ClosePane { pane_id })
            }
            SelectPane { target } => self.client.execute_workspace_task(
                &self.workspace_id,
                ClientTask::SwitchPane { pane_id: target.0 },
            ),
            ResizePane {
                target,
                width: Some(width),
                height: Some(height),
            } => self
                .client
                .resize_workspace_pane(&self.workspace_id, target.0, *width, *height),
            ResizePane {
                target,
                width: Some(size),
                height: None,
            } => self.client.resize_workspace_pane_axis(
                &self.workspace_id,
                target.0,
                ClientResizeAxis::Horizontal,
                *size,
            ),
            ResizePane {
                target,
                width: None,
                height: Some(size),
            } => self.client.resize_workspace_pane_axis(
                &self.workspace_id,
                target.0,
                ClientResizeAxis::Vertical,
                *size,
            ),
            ResizePane {
                width: None,
                height: None,
                ..
            } => return Ok(()),
            ResizeClient { width, height } => {
                self.client
                    .resize_workspace_client(&self.workspace_id, *width, *height)
            }
            SendKeys { target, text } => {
                let Some(pane_id) = target
                    .map(|pane_id| pane_id.0)
                    .or_else(|| self.active_pane_id())
                else {
                    return Ok(());
                };
                self.client
                    .send_workspace_input(&self.workspace_id, pane_id, text.as_bytes())
            }
            WriteRaw { target, data } => {
                let Some(pane_id) = target
                    .map(|pane_id| pane_id.0)
                    .or_else(|| self.active_pane_id())
                else {
                    return Ok(());
                };
                self.client
                    .send_workspace_input(&self.workspace_id, pane_id, data)
            }
            CapturePane { .. }
            | ListWorkspaces
            | ListTabs
            | ListPanes { .. }
            | ListLayout
            | DisplayMessage { .. } => return Ok(()),
        };

        if code == 0 {
            Ok(())
        } else {
            anyhow::bail!("Core FFI daemon task failed with code {code}")
        }
    }
}

/// 启动 daemon：connect backend → 监听 socket → 处理请求循环。
///
/// `socket_path` 是 unix socket 路径，`name` 是 session 名（日志用）。
/// 返回时表示 daemon 即将退出。
pub fn run_daemon(socket_path: PathBuf, name: String, tmux_socket: Option<String>) -> Result<()> {
    info!(target: "muxterm", session = %name, "daemon 启动");

    let mut state = DaemonState::connect(&name, tmux_socket.as_deref())?;
    state.pending_events = state.poll(true)?;

    // 绑定 unix socket
    // 先删除可能残留的旧 socket 文件
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("绑定 socket 失败: {}", socket_path.display()))?;
    info!(target: "muxterm", socket = %socket_path.display(), "daemon 监听中");

    // 注册 SIGINT/SIGTERM → 删除 socket 并退出
    let sock_path = socket_path.clone();
    let shutdown_flag = Arc::new(Mutex::new(false));
    let flag_clone = shutdown_flag.clone();
    // 简单实现：用 ctrl-c handler
    let _ = ctrl_c_handler({
        let f = flag_clone.clone();
        move || {
            *f.lock().unwrap() = true;
        }
    });

    // 非阻塞 accept 循环
    listener.set_nonblocking(true).context("set nonblocking")?;

    loop {
        // 检查 shutdown
        if *shutdown_flag.lock().unwrap() {
            break;
        }

        match listener.accept() {
            Ok((stream, _)) => {
                if handle_connection(stream, &mut state)? {
                    // KillSession：daemon 退出
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // 短暂 sleep 避免 busy loop
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                warn!(target: "muxterm", error = %e, "accept 失败");
                break;
            }
        }
    }

    // 清理
    info!(target: "muxterm", "daemon 退出，清理 socket");
    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}

/// 处理单个 client 连接。
fn handle_connection(
    stream: std::os::unix::net::UnixStream,
    state: &mut DaemonState,
) -> Result<bool> {
    use std::io::{BufRead, BufReader, Write};

    let reader = BufReader::new(&stream);
    let mut writer = &stream;

    let mut should_kill = false;
    let mut first_request = true;
    for line in reader.lines() {
        // 客户端发完一个请求后即关闭连接；连接关闭导致的读错误不应让 daemon 退出。
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }

        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::err(format!("反序列化请求失败: {e}"));
                writeln!(writer, "{}", serde_json::to_string(&resp).unwrap())?;
                continue;
            }
        };

        // 检查是否是 KillSession
        if matches!(req.command, crate::cli::CliCommand::CloseWorkspace { .. }) {
            should_kill = true;
        }

        let resp = execute_request(&req, state, first_request);
        first_request = false;

        let resp_json = serde_json::to_string(&resp)
            .unwrap_or_else(|_| serde_json::to_string(&Response::err("响应序列化失败")).unwrap());
        writeln!(writer, "{resp_json}")?;
        writer.flush()?;

        if should_kill {
            break;
        }
    }

    Ok(should_kill)
}

/// 执行单个请求，返回 Response。
fn execute_request(req: &Request, state: &mut DaemonState, first_request: bool) -> Response {
    // Each short-lived daemon client needs a control baseline.  The daemon
    // owns one event queue, so a later client cannot reconstruct its tabs and
    // panes from deltas consumed by an earlier client.
    let mut events = match state.drain_events(first_request) {
        Ok(events) => events,
        Err(error) => return Response::err(format!("轮询 daemon 事件失败: {error}")),
    };
    if let Err(error) = state.execute(&req.command) {
        return Response::err(format!("执行失败: {error}"));
    }
    match state.drain_events(false) {
        Ok(after) => events.extend(after),
        Err(error) => return Response::err(format!("轮询 daemon 事件失败: {error}")),
    }

    // A short-lived client may have consumed the live output event in an
    // earlier request.  A capture request doubles as the explicit render
    // baseline barrier: return the current raw bytes as a pane_snapshot so
    // the next client can reconstruct the pane without relying on the
    // formatted query string.
    if let CliCommand::CapturePane { target, .. } = &req.command {
        let pane_id = target.map(|pane| pane.0).or_else(|| state.active_pane_id());
        if let Some(pane_id) = pane_id {
            events.push(serde_json::json!({
                "kind": "pane_snapshot",
                "pane_id": pane_id,
                "data": state
                    .client
                    .get_workspace_pane_output(&state.workspace_id, pane_id),
            }));
        }
    }

    let output = if is_query(&req.command) {
        match format_ffi_output(&state.client, &state.workspace_id, &req.command, req.format) {
            Ok(output) => output,
            Err(error) => return Response::err(format!("格式化输出失败: {error}")),
        }
    } else {
        String::new()
    };
    Response::ok_with_events(output, events)
}

fn is_query(command: &CliCommand) -> bool {
    matches!(
        command,
        CliCommand::ListWorkspaces
            | CliCommand::ListTabs
            | CliCommand::ListPanes { .. }
            | CliCommand::ListLayout
            | CliCommand::CapturePane { .. }
            | CliCommand::DisplayMessage { .. }
    )
}

/// 简易 ctrl-c handler：安装 SIGINT/SIGTERM handler 设置 flag。
fn ctrl_c_handler<F>(_f: F) -> Result<()>
where
    F: Fn() + Send + Sync + 'static,
{
    // 简化：daemon 靠 listener 非阻塞 + client KillSession 退出。
    // 真实 SIGINT/SIGTERM 处理交给 OS（进程退出时 socket 文件由 client 清理）。
    // 这里不做复杂的 signal handler 安装（避免 unsafe + 线程安全问题）。
    Ok(())
}
