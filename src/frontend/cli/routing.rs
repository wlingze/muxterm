//! CLI 路由：从 main.rs 提取的命令分发逻辑。

use std::time::{Duration, Instant};

use crate::frontend::cli::{format_ffi_output, parse_cli_command, CliCommand, OutputFormat};
use crate::frontend::ffi_client::{ClientEventKind, ClientResizeAxis, ClientTask, FfiClient};

/// CLI 命令模式入口：解析命令 → 路由 → 执行 → 输出。
pub fn run_cli(args: &[String]) -> anyhow::Result<()> {
    // tmux CLI 结构化命令：muxterm tmux session/tab/pane ...
    if args.first().map(|s| s.as_str()) == Some("tmux") {
        return crate::frontend::cli::tmux_cli_exec::run_tmux_cli(&args[1..]);
    }

    let (cmd, format_str) = parse_cli_command(args)?;
    let format = format_str
        .map(|s| OutputFormat::from_str(&s))
        .unwrap_or(OutputFormat::Json);

    if let CliCommand::Config { args } = &cmd {
        return match crate::frontend::cli::config::run(args, format) {
            Ok(()) => Ok(()),
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(crate::frontend::cli::config::exit_code(&error));
            }
        };
    }

    let socket = extract_socket_arg(args);
    let session_name = extract_session_name(&cmd, args);

    match (session_name, socket.is_some()) {
        (Some(name), true) => cli_mode_daemon(&name, &cmd, format, socket.as_deref()),
        (Some(name), false) => cli_mode_daemon(&name, &cmd, format, None),
        (None, true) => cli_mode_tmux(socket.as_deref(), None, &cmd, format),
        (None, false) => cli_mode_ephemeral(&cmd, format),
    }
}

/// 从命令参数中提取 -L <socket>。
fn extract_socket_arg(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-L" {
            if let Some(name) = iter.next() {
                return Some(name.clone());
            }
        }
    }
    None
}

/// 从命令参数中提取 session name（`-s <name>`）。
fn extract_session_name(cmd: &CliCommand, args: &[String]) -> Option<String> {
    if let CliCommand::NewWorkspace { socket, .. } = cmd {
        if socket.is_some() {
            return socket.clone();
        }
    }
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-s" {
            if let Some(name) = iter.next() {
                return Some(name.clone());
            }
        }
    }
    None
}

/// A short-lived CLI owner for one Core FFI handle and one workspace.
///
/// The CLI has no resident Scene, so it owns the handle only for the duration
/// of one command.  Runtime construction, workspace selection, task routing,
/// and snapshot queries all stay behind [`FfiClient`].
struct FfiCliSession {
    client: FfiClient,
    workspace_id: String,
}

impl FfiCliSession {
    fn local() -> anyhow::Result<Self> {
        Self::connect("local", None, None)
    }

    fn tmux(
        socket: Option<&str>,
        session_name: Option<&str>,
        cmd: &CliCommand,
    ) -> anyhow::Result<Self> {
        let attach_target = match cmd {
            CliCommand::AttachWorkspace { target } => Some(target.as_str()),
            _ => session_name,
        };
        let selected_session = if let Some(target) = attach_target {
            let exists = FfiClient::discover_tmux_sessions("local", None, socket)?
                .iter()
                .any(|session| session.name == target);
            if !exists && !matches!(cmd, CliCommand::AttachWorkspace { .. }) {
                FfiClient::create_workspace("local", None, socket, target, ".")?;
            }
            Some(target.to_string())
        } else {
            FfiClient::discover_tmux_sessions("local", None, socket)?
                .first()
                .map(|session| session.name.clone())
        };
        Self::connect("tmux", socket, selected_session.as_deref())
    }

    fn connect(runtime: &str, socket: Option<&str>, session: Option<&str>) -> anyhow::Result<Self> {
        let start_directory = (runtime == "local").then_some("");
        let client = FfiClient::new_connect(runtime, socket, session, None, start_directory)?;
        let workspaces = client.workspace_list()?;
        let workspace = workspaces
            .iter()
            .find(|workspace| workspace.active)
            .or_else(|| workspaces.first())
            .ok_or_else(|| anyhow::anyhow!("Core returned no CLI workspace"))?;
        Ok(Self {
            client,
            workspace_id: workspace.id.clone(),
        })
    }

    fn poll(&self) {
        let _ = self.client.poll_workspace_events();
    }

    fn wait(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.poll();
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_for_pane_snapshot(&self, pane_id: u32, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let events = self.client.poll_workspace_events();
            if events.iter().any(|event| {
                event.event.pane_id == pane_id
                    && matches!(event.event.kind(), ClientEventKind::PaneSnapshot)
            }) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn prepare_for_command(&self, cmd: &CliCommand) {
        if !matches!(
            cmd,
            CliCommand::SendKeys { .. }
                | CliCommand::WriteRaw { .. }
                | CliCommand::CapturePane { .. }
        ) {
            return;
        }
        let pane_id = match cmd {
            CliCommand::SendKeys { target, .. }
            | CliCommand::WriteRaw { target, .. }
            | CliCommand::CapturePane { target, .. } => {
                target.map(|id| id.0).or_else(|| self.active_pane_id())
            }
            _ => None,
        };
        if let Some(pane_id) = pane_id {
            let code = self.client.execute_workspace_task(
                &self.workspace_id,
                ClientTask::RequestPaneSnapshot { pane_id },
            );
            if code == 0 {
                self.wait_for_pane_snapshot(pane_id, Duration::from_secs(2));
            }
        }
    }

    fn active_tab_id(&self) -> Option<u32> {
        self.client
            .get_workspace_tabs(&self.workspace_id)
            .into_iter()
            .find(|tab| tab.is_active)
            .map(|tab| tab.id)
    }

    fn active_pane_id(&self) -> Option<u32> {
        self.client
            .get_workspace_tabs(&self.workspace_id)
            .into_iter()
            .find(|tab| tab.is_active)
            .and_then(|tab| {
                self.client
                    .get_workspace_panes(&self.workspace_id, tab.id)
                    .into_iter()
                    .find(|pane| pane.is_active)
                    .map(|pane| pane.id)
            })
    }

    fn execute(&self, task: ClientTask) -> anyhow::Result<()> {
        let code = self.client.execute_workspace_task(&self.workspace_id, task);
        Self::check_code("task", code)
    }

    fn check_code(operation: &str, code: i32) -> anyhow::Result<()> {
        if code == 0 {
            Ok(())
        } else {
            anyhow::bail!("Core FFI {operation} failed with code {code}")
        }
    }

    fn execute_command(&self, cmd: &CliCommand) -> anyhow::Result<()> {
        use CliCommand::*;

        match cmd {
            Config { .. }
            | NewWorkspace { .. }
            | AttachWorkspace { .. }
            | ListWorkspaces
            | ListTabs
            | ListPanes { .. }
            | ListLayout
            | CapturePane { .. }
            | DisplayMessage { .. }
            | PollEvents => Ok(()),
            CloseWorkspace { .. } => self.execute(ClientTask::Shutdown),
            Detach { .. } => self.execute(ClientTask::Detach),
            RenameWorkspace { new_name } => Self::check_code(
                "rename workspace",
                self.client.rename_workspace(&self.workspace_id, new_name),
            ),
            NewTab { name } => Self::check_code(
                "new tab",
                self.client
                    .new_workspace_tab(&self.workspace_id, name.as_deref()),
            ),
            KillTab { target } => {
                let tab_id = target
                    .map(|id| id.0)
                    .or_else(|| self.active_tab_id())
                    .ok_or_else(|| anyhow::anyhow!("Core returned no active tab"))?;
                self.execute(ClientTask::CloseTab { tab_id })
            }
            SelectTab { target } => self.execute(ClientTask::SwitchTab { tab_id: target.0 }),
            RenameTab { new_name } => {
                let tab_id = self
                    .active_tab_id()
                    .ok_or_else(|| anyhow::anyhow!("Core returned no active tab"))?;
                Self::check_code(
                    "rename tab",
                    self.client
                        .rename_workspace_tab(&self.workspace_id, tab_id, new_name),
                )
            }
            SplitPane {
                horizontal, target, ..
            } => {
                let pane_id = target
                    .map(|id| id.0)
                    .or_else(|| self.active_pane_id())
                    .ok_or_else(|| anyhow::anyhow!("Core returned no active pane"))?;
                self.execute(ClientTask::SplitPane {
                    pane_id,
                    horizontal: *horizontal,
                })
            }
            KillPane { target } => {
                let pane_id = target
                    .map(|id| id.0)
                    .or_else(|| self.active_pane_id())
                    .ok_or_else(|| anyhow::anyhow!("Core returned no active pane"))?;
                self.execute(ClientTask::ClosePane { pane_id })
            }
            SelectPane { target } => self.execute(ClientTask::SwitchPane { pane_id: target.0 }),
            ResizePane {
                target,
                width,
                height,
            } => match (width, height) {
                (Some(cols), Some(rows)) => Self::check_code(
                    "resize pane",
                    self.client
                        .resize_workspace_pane(&self.workspace_id, target.0, *cols, *rows),
                ),
                (Some(size), None) => Self::check_code(
                    "resize pane axis",
                    self.client.resize_workspace_pane_axis(
                        &self.workspace_id,
                        target.0,
                        ClientResizeAxis::Horizontal,
                        *size,
                    ),
                ),
                (None, Some(size)) => Self::check_code(
                    "resize pane axis",
                    self.client.resize_workspace_pane_axis(
                        &self.workspace_id,
                        target.0,
                        ClientResizeAxis::Vertical,
                        *size,
                    ),
                ),
                (None, None) => anyhow::bail!("resize pane requires a size"),
            },
            ResizeClient { width, height } => Self::check_code(
                "resize client",
                self.client
                    .resize_workspace_client(&self.workspace_id, *width, *height),
            ),
            SendKeys { target, text } => {
                let pane_id = target
                    .map(|id| id.0)
                    .or_else(|| self.active_pane_id())
                    .ok_or_else(|| anyhow::anyhow!("Core returned no active pane"))?;
                Self::check_code(
                    "send keys",
                    self.client
                        .send_workspace_input(&self.workspace_id, pane_id, text.as_bytes()),
                )
            }
            WriteRaw { target, data } => {
                let pane_id = target
                    .map(|id| id.0)
                    .or_else(|| self.active_pane_id())
                    .ok_or_else(|| anyhow::anyhow!("Core returned no active pane"))?;
                Self::check_code(
                    "write raw",
                    self.client
                        .send_workspace_input(&self.workspace_id, pane_id, data),
                )
            }
        }
    }

    fn output(&self, cmd: &CliCommand, format: OutputFormat) -> anyhow::Result<String> {
        format_ffi_output(&self.client, &self.workspace_id, cmd, format)
    }

    fn shutdown(&self) {
        let _ = self.client.shutdown();
    }
}

/// tmux mode: connect through the public FFI, execute one command, then close
/// only this short-lived control client.
fn cli_mode_tmux(
    socket: Option<&str>,
    session_name: Option<&str>,
    cmd: &CliCommand,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let session = FfiCliSession::tmux(socket, session_name, cmd)?;
    session.wait(Duration::from_millis(500));
    session.prepare_for_command(cmd);
    session.execute_command(cmd)?;
    session.wait(command_settle_duration(cmd));

    let output = session.output(cmd, format)?;
    if !output.is_empty() {
        println!("{output}");
    }

    session.shutdown();
    Ok(())
}

fn command_settle_duration(cmd: &CliCommand) -> Duration {
    if matches!(
        cmd,
        CliCommand::SendKeys { .. } | CliCommand::WriteRaw { .. }
    ) {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(500)
    }
}

/// daemon 模式：连接/启动 daemon，发送命令，打印输出。
fn cli_mode_daemon(
    name: &str,
    cmd: &CliCommand,
    format: OutputFormat,
    tmux_socket: Option<&str>,
) -> anyhow::Result<()> {
    use crate::frontend::cli::session::session_socket_path;

    let sock = session_socket_path(name);

    if matches!(cmd, CliCommand::NewWorkspace { .. }) {
        if sock.exists() && !socket_is_alive(&sock) {
            let _ = std::fs::remove_file(&sock);
        }
        if !sock.exists() {
            spawn_daemon(&sock, name, tmux_socket)?;
            wait_for_socket(&sock, std::time::Duration::from_secs(5))?;
        }
        return Ok(());
    }

    if !sock.exists() {
        anyhow::bail!(
            "session '{}' 不存在（socket: {}）。用 `muxterm new-session -s {}` 创建。",
            name,
            sock.display(),
            name
        );
    }

    let socket = sock.to_string_lossy().into_owned();
    let session = FfiCliSession::connect("daemon", Some(&socket), Some(name))?;
    session.wait(Duration::from_millis(500));
    session.prepare_for_command(cmd);
    session.execute_command(cmd)?;
    if !matches!(cmd, CliCommand::CloseWorkspace { .. }) {
        session.wait(command_settle_duration(cmd));
    }

    let output = session.output(cmd, format)?;
    if !output.is_empty() {
        println!("{output}");
    }
    session.shutdown();

    Ok(())
}

/// fork 启动 daemon 进程（后台）。
pub(crate) fn spawn_daemon(
    socket_path: &std::path::Path,
    name: &str,
    tmux_socket: Option<&str>,
) -> anyhow::Result<()> {
    use crate::frontend::cli::daemon::run_daemon;

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        anyhow::bail!("fork 失败");
    }
    if pid > 0 {
        return Ok(());
    }

    unsafe {
        libc::setsid();
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if devnull >= 0 {
            let _ = libc::dup2(devnull, libc::STDIN_FILENO);
            let _ = libc::dup2(devnull, libc::STDOUT_FILENO);
            let _ = libc::dup2(devnull, libc::STDERR_FILENO);
            if devnull > libc::STDERR_FILENO {
                let _ = libc::close(devnull);
            }
        }
    }

    if let Err(_e) = run_daemon(
        socket_path.to_path_buf(),
        name.to_string(),
        tmux_socket.map(|s| s.to_string()),
    ) {
        std::process::exit(1);
    }
    std::process::exit(0);
}

/// socket 文件存在且可 connect。
pub(crate) fn socket_is_alive(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// 等待 socket 可连接（轮询）。
pub(crate) fn wait_for_socket(
    path: &std::path::Path,
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if socket_is_alive(path) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    anyhow::bail!("等待 daemon 启动超时: {}", path.display())
}

/// TUI × local：若 daemon 不存在则 fork 启动。
pub fn ensure_local_daemon(name: &str) -> anyhow::Result<()> {
    use crate::frontend::cli::session::session_socket_path;
    let sock = session_socket_path(name);
    if sock.exists() && !socket_is_alive(&sock) {
        let _ = std::fs::remove_file(&sock);
    }
    if sock.exists() {
        return Ok(());
    }
    spawn_daemon(&sock, name, None)?;
    wait_for_socket(&sock, std::time::Duration::from_secs(5))?;
    Ok(())
}

/// Ephemeral mode: create a local shell workspace through FFI, execute once,
/// then release the handle.
fn cli_mode_ephemeral(cmd: &CliCommand, format: OutputFormat) -> anyhow::Result<()> {
    let session = FfiCliSession::local()?;
    session.wait(Duration::from_millis(500));
    session.prepare_for_command(cmd);
    session.execute_command(cmd)?;
    session.wait(command_settle_duration(cmd));

    let output = session.output(cmd, format)?;
    if !output.is_empty() {
        println!("{output}");
    }

    session.shutdown();
    Ok(())
}
