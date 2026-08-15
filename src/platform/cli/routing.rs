//! CLI 路由：从 main.rs 提取的命令分发逻辑。

use crate::core::model::Backend;
use crate::core::model::TerminalModel;
use crate::core::runtime::shell::LocalBackend;
use crate::core::runtime::tmux::TmuxBackend;
use crate::platform::cli::entry::cli_command_to_task;
use crate::platform::cli::{format_output, parse_cli_command, CliCommand, OutputFormat};

/// CLI 命令模式入口：解析命令 → 路由 → 执行 → 输出。
pub fn run_cli(args: &[String]) -> anyhow::Result<()> {
    // tmux CLI 结构化命令：muxterm tmux session/tab/pane ...
    if args.first().map(|s| s.as_str()) == Some("tmux") {
        return crate::platform::cli::tmux_cli_exec::run_tmux_cli(&args[1..]);
    }

    let (cmd, format_str) = parse_cli_command(args)?;
    let format = format_str
        .map(|s| OutputFormat::from_str(&s))
        .unwrap_or(OutputFormat::Json);

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

/// 用 `tmux -L <socket> list-sessions` 查找已有的 session 名。
fn find_existing_tmux_session(socket: Option<&str>) -> Option<String> {
    let mut cmd = std::process::Command::new("tmux");
    if let Some(s) = socket {
        cmd.args(["-L", s]);
    }
    cmd.args(["list-sessions", "-F", "#{session_name}"]);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let output = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().next().map(|s| s.trim().to_string())
}

/// tmux 模式：用 TmuxBackend 连接 tmux server，执行命令后关闭。
fn cli_mode_tmux(
    socket: Option<&str>,
    session_name: Option<&str>,
    cmd: &CliCommand,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let backend: Box<dyn Backend> = match cmd {
        CliCommand::AttachWorkspace { target } => {
            Box::new(TmuxBackend::new_with_attach(socket, target))
        }
        _ => {
            if let Some(name) = session_name {
                let existing = find_existing_tmux_session(socket);
                if existing.as_deref() == Some(name) {
                    Box::new(TmuxBackend::new_with_attach(socket, name))
                } else {
                    Box::new(TmuxBackend::new_with_session_name(socket, name))
                }
            } else {
                let existing_session = find_existing_tmux_session(socket);
                if let Some(name) = existing_session {
                    Box::new(TmuxBackend::new_with_attach(socket, &name))
                } else {
                    Box::new(TmuxBackend::new(socket))
                }
            }
        }
    };
    let mut model = TerminalModel::new(backend);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()?;
    rt.block_on(model.connect())?;
    let _ = model.poll_events();

    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = model.refresh();

    let task = cli_command_to_task(cmd, model.state());
    if let Some(t) = task {
        model.execute(t)?;
        let _ = model.poll_events();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = model.refresh();
    }

    let output = format_output(model.state(), cmd, format);
    if !output.is_empty() {
        println!("{output}");
    }

    let _ = rt.block_on(model.shutdown());
    Ok(())
}

/// daemon 模式：连接/启动 daemon，发送命令，打印输出。
fn cli_mode_daemon(
    name: &str,
    cmd: &CliCommand,
    format: OutputFormat,
    tmux_socket: Option<&str>,
) -> anyhow::Result<()> {
    use crate::platform::cli::client::send_command;
    use crate::platform::cli::session::session_socket_path;

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

    let resp = send_command(&sock, cmd, format)?;
    if resp.ok {
        if !resp.output.is_empty() {
            println!("{}", resp.output);
        }
    } else {
        eprintln!("错误: {}", resp.error);
        std::process::exit(1);
    }

    Ok(())
}

/// fork 启动 daemon 进程（后台）。
pub(crate) fn spawn_daemon(
    socket_path: &std::path::Path,
    name: &str,
    tmux_socket: Option<&str>,
) -> anyhow::Result<()> {
    use crate::platform::cli::daemon::run_daemon;

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
    use crate::platform::cli::session::session_socket_path;
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

/// 临时模式：创建临时 LocalBackend，执行后关闭。
fn cli_mode_ephemeral(cmd: &CliCommand, format: OutputFormat) -> anyhow::Result<()> {
    let backend = LocalBackend::new("$SHELL", "");
    let mut model = TerminalModel::new(Box::new(backend));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(model.connect())?;
    let _ = model.poll_events();

    let task = cli_command_to_task(cmd, model.state());
    if let Some(t) = task {
        model.execute(t)?;
        let _ = model.poll_events();
    }

    let output = format_output(model.state(), cmd, format);
    if !output.is_empty() {
        println!("{output}");
    }

    let _ = rt.block_on(model.shutdown());
    Ok(())
}
