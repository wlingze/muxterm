#![allow(dead_code)]
// Muxterm 主入口。
//
// 根据 Cargo feature flag + CLI flag 选择前端：
// - `gtk`：GTK4 原生前端
// - `tui`：纯 crossterm TUI 前端
// 不带 --tui/--gtk 时走 CLI 命令模式（不依赖任何 feature）。
// 至少要启用一个前端 feature，否则编译期报错（但 CLI 命令始终可用）。

use clap::Parser;

mod cli;
mod core;
mod main_entry;
mod platform;

/// Muxterm 命令行参数
#[derive(Parser, Debug)]
#[command(
    name = "muxterm",
    version,
    about = "Native UI terminal for tmux control mode"
)]
struct Cli {
    /// 启用详细日志（RUST_LOG 也可以控制）
    #[arg(short, long)]
    verbose: bool,

    /// tmux socket 名（传给 `tmux -L`，隔离独立 server，不影响默认会话）
    #[arg(short = 'L', long = "socket", value_name = "SOCKET")]
    socket: Option<String>,

    /// session 名（local 模式连 daemon；tmux 模式指定 attach 目标）
    #[arg(short = 's', long = "session", value_name = "NAME")]
    session: Option<String>,

    /// 使用 ASCII TUI 前端（而非 GTK4）。需要启用 `tui` feature。
    #[arg(long = "tui", default_value_t = false)]
    tui: bool,

    /// 使用 GTK4 前端（默认）。需要启用 `gtk` feature。
    #[arg(long = "gtk", default_value_t = false)]
    gtk: bool,
}

fn main() -> anyhow::Result<()> {
    // 检查是否有子命令（CLI 命令模式）
    let raw_args: Vec<String> = std::env::args().collect();

    // 如果第一个非程序名参数是已知 CLI 命令（不以 - 开头），走 CLI 命令模式
    if let Some(first) = raw_args.get(1) {
        if !first.starts_with('-') && !first.is_empty() {
            // CLI 命令模式
            let cmd_args: Vec<String> = raw_args[1..].to_vec();
            return cli_mode(&cmd_args);
        }
    }

    // 交互模式（--tui 或 --gtk）
    #[cfg(not(any(feature = "gtk", feature = "tui")))]
    {
        eprintln!("muxterm: 没有可用的前端（需要 --features gtk 或 tui）");
        eprintln!("提示：用 `muxterm <command>` 执行 CLI 命令");
        std::process::exit(1);
    }

    let cli = Cli::parse();
    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    if let Some(ref sock) = cli.socket {
        tracing::info!(target = "muxterm", socket = %sock, "使用独立 tmux socket (-L)");
    }

    // 决定前端
    let want_tui = cli.tui || (!cli.gtk && cfg!(not(feature = "gtk")) && cfg!(feature = "tui"));
    let want_gtk = !want_tui && cfg!(feature = "gtk");

    if want_tui {
        #[cfg(feature = "tui")]
        {
            tracing::info!(target = "muxterm", "muxterm 启动（TUI）");
            // TUI × local（-s 无 -L）：确保 daemon 存在
            if let Some(ref name) = cli.session {
                if cli.socket.is_none() {
                    ensure_local_daemon(name)?;
                }
            }
            return platform::tui::app::run(platform::tui::app::TuiOpts {
                socket: cli.socket,
                session: cli.session,
            });
        }
        #[cfg(not(feature = "tui"))]
        {
            anyhow::bail!("TUI 前端未编译（需要 --features tui）");
        }
    }

    if want_gtk {
        #[cfg(feature = "gtk")]
        {
            tracing::info!(target = "muxterm", "muxterm 启动（GTK4 UI）");
            return platform::linux::app::run(cli.socket);
        }
        #[cfg(not(feature = "gtk"))]
        {
            anyhow::bail!("GTK4 前端未编译（需要 --features gtk）");
        }
    }

    anyhow::bail!("没有可用的前端（启用 `gtk` 或 `tui` feature）")
}

/// CLI 命令模式：解析命令 → 执行 → 格式化输出。
///
/// 两种路径：
/// 1. **daemon 模式**（`-s <name>` 或 `new-session -s <name>`）：
///    连接到持久 daemon 的 unix socket，命令在 daemon 内执行，状态保留。
///    `new-session` 时如果 daemon 不存在则 fork 启动。
/// 2. **临时模式**（无 `-s`）：创建临时 LocalBackend，执行后关闭（状态不保留）。
fn cli_mode(args: &[String]) -> anyhow::Result<()> {
    // tmux CLI 结构化命令：muxterm tmux session/tab/pane ...
    if args.first().map(|s| s.as_str()) == Some("tmux") {
        return cli::tmux_cli_exec::run_tmux_cli(&args[1..]);
    }

    use cli::{parse_cli_command, OutputFormat};

    let (cmd, format_str) = parse_cli_command(args)?;
    let format = format_str
        .map(|s| OutputFormat::from_str(&s))
        .unwrap_or(OutputFormat::Json);

    // 提取全局 -L <socket> 参数（CLI 命令模式下的 tmux socket）
    let socket = extract_socket_arg(args);

    // 判断是否走 daemon 模式
    let session_name = extract_session_name(&cmd, args);

    match (session_name, socket.is_some()) {
        // -s + -L → daemon 模式 + TmuxBackend（持久 daemon 通过 -CC 连接 tmux）
        (Some(name), true) => cli_mode_daemon(&name, &cmd, format, socket.as_deref()),
        // 只有 -s → daemon 模式（LocalBackend）
        (Some(name), false) => cli_mode_daemon(&name, &cmd, format, None),
        // 只有 -L → tmux 模式（临时连接，无 daemon）
        (None, true) => cli_mode_tmux(socket.as_deref(), None, &cmd, format),
        // 都没有 → 临时模式（LocalBackend）
        (None, false) => cli_mode_ephemeral(&cmd, format),
    }
}

/// 从命令参数中提取 -L <socket> 参数。
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

/// 用 `tmux -L <socket> list-sessions` 查找已有的 session 名。
/// 返回第一个 session 的名字（如果有）。
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
///
/// 连接模式选择：
/// - AttachSession { target } → attach 到 target session
/// - -s <name> → attach 到 <name> session（如果存在），否则 new-session -s <name>
/// - 无 -s → attach 到第一个已有 session，否则 new-session
fn cli_mode_tmux(
    socket: Option<&str>,
    session_name: Option<&str>,
    cmd: &cli::CliCommand,
    format: cli::OutputFormat,
) -> anyhow::Result<()> {
    use crate::core::backend::TmuxBackend;
    use crate::core::model::TerminalModel;
    use cli::format_output;

    // 根据命令选择连接模式
    let backend: Box<dyn crate::core::model::Backend> = match cmd {
        cli::CliCommand::AttachSession { target } => {
            // attach 模式：target 是 session 名或 $N 格式
            Box::new(TmuxBackend::new_with_attach(socket, target))
        }
        _ => {
            // 非 attach 命令
            if let Some(name) = session_name {
                // 有 -s <name>：检查 session 是否已存在
                let existing = find_existing_tmux_session(socket);
                if existing.as_deref() == Some(name) {
                    // session 已存在 → attach
                    Box::new(TmuxBackend::new_with_attach(socket, name))
                } else {
                    // session 不存在 → new-session -s <name>
                    Box::new(TmuxBackend::new_with_session_name(socket, name))
                }
            } else {
                // 无 -s：检查 tmux server 是否已有 session
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

    // 给后台 task 时间处理查询响应（list-sessions, list-windows 等）
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = model.refresh();

    let task = cli_command_to_task(cmd, model.state());
    if let Some(t) = task {
        model.execute(t)?;
        let _ = model.poll_events();
        // 等待操作结果
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

/// 从命令参数中提取 session name（`-s <name>`）。
///
/// `-s <name>` 是全局参数，可出现在任何命令的参数中（parse_cli_command 的
/// filter 只处理 --format，其余参数原样保留）。这里扫描原始 args 查找 -s。
fn extract_session_name(cmd: &cli::CliCommand, args: &[String]) -> Option<String> {
    // NewSession 的 -s 参数优先
    if let cli::CliCommand::NewSession { socket, .. } = cmd {
        if socket.is_some() {
            return socket.clone();
        }
    }
    // 全局 -s 参数：扫描原始 args
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

/// daemon 模式：连接/启动 daemon，发送命令，打印输出。
fn cli_mode_daemon(
    name: &str,
    cmd: &cli::CliCommand,
    format: cli::OutputFormat,
    tmux_socket: Option<&str>,
) -> anyhow::Result<()> {
    use cli::client::send_command;
    use cli::session::session_socket_path;

    let sock = session_socket_path(name);

    // NewSession：启动 daemon；若残留死 socket 则清掉再起
    if matches!(cmd, cli::CliCommand::NewSession { .. }) {
        if sock.exists() && !socket_is_alive(&sock) {
            tracing::warn!(
                target: "muxterm",
                socket = %sock.display(),
                "发现残留死 socket，删除后重建 daemon"
            );
            let _ = std::fs::remove_file(&sock);
        }
        if !sock.exists() {
            spawn_daemon(&sock, name, tmux_socket)?;
            // 等待 daemon 就绪（最多 5 秒，tmux -CC 启动较慢）
            wait_for_socket(&sock, std::time::Duration::from_secs(5))?;
        }
        tracing::info!(target: "muxterm", session = %name, "session 已创建");
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

    // 发送命令到 daemon
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
fn spawn_daemon(
    socket_path: &std::path::Path,
    name: &str,
    tmux_socket: Option<&str>,
) -> anyhow::Result<()> {
    use cli::daemon::run_daemon;

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        anyhow::bail!("fork 失败");
    }
    if pid > 0 {
        tracing::info!(target: "muxterm", pid = pid, "daemon 进程已 fork");
        return Ok(());
    }

    // 子进程：脱离终端，并关掉从 parent Command 继承的 stdout/stderr pipe。
    // 否则调用方的 `.output()` 会一直等 EOF（daemon 仍握着 pipe 写端）。
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

/// socket 文件存在且可 connect（排除残留死 socket）。
fn socket_is_alive(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// 等待 socket 可连接（轮询）。
fn wait_for_socket(path: &std::path::Path, timeout: std::time::Duration) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if socket_is_alive(path) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    anyhow::bail!("等待 daemon 启动超时: {}", path.display())
}

/// TUI × local：若 daemon 不存在则 fork 启动（等价于先 new-session）。
fn ensure_local_daemon(name: &str) -> anyhow::Result<()> {
    use cli::session::session_socket_path;
    let sock = session_socket_path(name);
    if sock.exists() && !socket_is_alive(&sock) {
        let _ = std::fs::remove_file(&sock);
    }
    if sock.exists() {
        return Ok(());
    }
    tracing::info!(target: "muxterm", session = %name, "TUI 启动前创建 local daemon");
    spawn_daemon(&sock, name, None)?;
    wait_for_socket(&sock, std::time::Duration::from_secs(5))?;
    Ok(())
}

/// 临时模式：创建临时 LocalBackend，执行后关闭（状态不保留）。
fn cli_mode_ephemeral(cmd: &cli::CliCommand, format: cli::OutputFormat) -> anyhow::Result<()> {
    use crate::core::backend::LocalBackend;
    use crate::core::model::TerminalModel;
    use cli::format_output;

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

/// 把 CliCommand 转成 TerminalModel 的 Task（委托给 main_entry 模块，供 daemon 复用）。
fn cli_command_to_task(
    cmd: &cli::CliCommand,
    state: &dyn crate::core::model::state::State,
) -> Option<crate::core::model::task::Task> {
    crate::main_entry::cli_command_to_task(cmd, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_short_l_socket() {
        let cli = Cli::try_parse_from(["muxterm", "-L", "muxterm-dev"]).unwrap();
        assert_eq!(cli.socket.as_deref(), Some("muxterm-dev"));
        assert!(!cli.verbose);
        assert!(!cli.tui);
        assert!(!cli.gtk);
    }

    #[test]
    fn cli_parses_long_socket() {
        let cli = Cli::try_parse_from(["muxterm", "--socket", "iso"]).unwrap();
        assert_eq!(cli.socket.as_deref(), Some("iso"));
    }

    #[test]
    fn cli_parses_socket_with_verbose() {
        let cli = Cli::try_parse_from(["muxterm", "-v", "-L", "dev"]).unwrap();
        assert!(cli.verbose);
        assert_eq!(cli.socket.as_deref(), Some("dev"));
    }

    #[test]
    fn cli_socket_defaults_to_none() {
        let cli = Cli::try_parse_from(["muxterm"]).unwrap();
        assert!(cli.socket.is_none());
        assert!(!cli.verbose);
        assert!(!cli.tui);
        assert!(!cli.gtk);
    }

    #[test]
    fn cli_parses_tui_flag() {
        let cli = Cli::try_parse_from(["muxterm", "--tui"]).unwrap();
        assert!(cli.tui);
        assert!(!cli.gtk);
    }

    #[test]
    fn cli_parses_session_short_s() {
        let cli = Cli::try_parse_from(["muxterm", "--tui", "-s", "mywork"]).unwrap();
        assert!(cli.tui);
        assert_eq!(cli.session.as_deref(), Some("mywork"));
        assert!(cli.socket.is_none());
    }

    #[test]
    fn cli_parses_tui_with_socket_and_session() {
        let cli = Cli::try_parse_from(["muxterm", "--tui", "-L", "test1", "-s", "demo"]).unwrap();
        assert!(cli.tui);
        assert_eq!(cli.socket.as_deref(), Some("test1"));
        assert_eq!(cli.session.as_deref(), Some("demo"));
    }

    #[test]
    fn cli_parses_gtk_flag() {
        let cli = Cli::try_parse_from(["muxterm", "--gtk"]).unwrap();
        assert!(cli.gtk);
        assert!(!cli.tui);
    }
}
