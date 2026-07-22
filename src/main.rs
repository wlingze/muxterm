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
            return platform::tui::app::run(cli.socket);
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

/// CLI 命令模式：解析命令 → TerminalModel::execute → 格式化输出。
fn cli_mode(args: &[String]) -> anyhow::Result<()> {
    use crate::core::backend::LocalBackend;
    use crate::core::model::task::Task;
    use crate::core::model::TerminalModel;
    use cli::{format_output, parse_cli_command, CliCommand, OutputFormat};

    let (cmd, format_str) = parse_cli_command(args)?;
    let format = format_str
        .map(|s| OutputFormat::from_str(&s))
        .unwrap_or(OutputFormat::Json);

    // CLI 命令用 LocalBackend（本地 shell）
    // 每个 CLI 命令独立连接 → 执行 → 输出 → 关闭
    // 对于查询命令（list-*），需要先 connect 建立 session
    let backend = LocalBackend::new("$SHELL", "");
    let mut model = TerminalModel::new(Box::new(backend));

    // 对所有命令都 connect：查询命令需要 state（connect 后才有
    // session/window/pane），操作命令需要 session 才能执行 Task。
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(model.connect())?;
    let _ = model.poll_events();

    // 操作类命令转成 Task 执行（查询命令 cli_command_to_task 返回 None，跳过）
    let task = cli_command_to_task(&cmd, &model);
    if let Some(t) = task {
        model.execute(t)?;
        let _ = model.poll_events();
    }

    // 格式化输出
    let output = format_output(model.state(), &cmd, format);
    if !output.is_empty() {
        println!("{output}");
    }

    // 关闭
    let _ = rt.block_on(model.shutdown());

    Ok(())
}

/// 把 CliCommand 转成 TerminalModel 的 Task。
fn cli_command_to_task(
    cmd: &cli::CliCommand,
    model: &crate::core::model::TerminalModel,
) -> Option<crate::core::model::task::Task> {
    use crate::core::model::layout::SplitDir;
    use crate::core::model::task::Task;
    use cli::CliCommand::*;

    match cmd {
        // Session
        NewSession { .. } => Some(Task::NewWindow {
            name: None,
            command: None,
            workdir: None,
        }),
        KillSession { .. } => Some(Task::Shutdown),
        AttachSession { .. } => None, // 暂不支持
        Detach { .. } => None,
        RenameSession { .. } => None,

        // Window
        NewWindow { name, .. } => Some(Task::NewWindow {
            name: name.clone(),
            command: None,
            workdir: None,
        }),
        KillWindow { target } => {
            let wid = target.or_else(|| model.state().active_window().map(|w| w.id))?;
            Some(Task::CloseWindow { target: wid })
        }
        SelectWindow { target } => Some(Task::SwitchWindow { target: *target }),
        RenameWindow { new_name } => {
            let wid = model.state().active_window()?.id;
            Some(Task::RenameWindow {
                target: wid,
                name: new_name.clone(),
            })
        }

        // Tab
        NewTab { name, window } => {
            let wid = window.or_else(|| model.state().active_window().map(|w| w.id))?;
            Some(Task::NewTab {
                window: wid,
                name: name.clone(),
                command: None,
                workdir: None,
            })
        }
        KillTab { target } => {
            let tid = target.or_else(|| model.state().active_tab().map(|t| t.id))?;
            Some(Task::CloseTab { target: tid })
        }
        SelectTab { target } => Some(Task::SwitchTab { target: *target }),
        RenameTab { new_name } => {
            let tid = model.state().active_tab()?.id;
            Some(Task::RenameTab {
                target: tid,
                name: new_name.clone(),
            })
        }

        // Pane
        SplitPane {
            horizontal, target, ..
        } => {
            let pid = target.or_else(|| model.state().active_pane().map(|p| p.id));
            let dir = if *horizontal {
                SplitDir::Horizontal
            } else {
                SplitDir::Vertical
            };
            Some(Task::SplitPane {
                target: pid,
                dir,
                command: None,
                workdir: None,
            })
        }
        KillPane { target } => {
            let pid = target.or_else(|| model.state().active_pane().map(|p| p.id))?;
            Some(Task::ClosePane { target: pid })
        }
        SelectPane { target } => Some(Task::SwitchPane { target: *target }),
        ResizePane {
            target,
            width,
            height,
        } => {
            let cols = width.unwrap_or(80);
            let rows = height.unwrap_or(24);
            Some(Task::ResizePane {
                target: *target,
                cols,
                rows,
            })
        }

        // 输入输出
        SendKeys { target, text } => {
            let pid = target.or_else(|| model.state().active_pane().map(|p| p.id))?;
            use crate::core::terminal::input::KeyEvent;
            let keys = text.chars().map(KeyEvent::Char).collect();
            Some(Task::SendKeys { target: pid, keys })
        }
        CapturePane { .. } => None, // 查询命令

        // 查询命令
        ListSessions
        | ListWindows { .. }
        | ListTabs { .. }
        | ListPanes { .. }
        | ListLayout { .. } => None,
        DisplayMessage { .. } => None,
    }
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
    fn cli_parses_gtk_flag() {
        let cli = Cli::try_parse_from(["muxterm", "--gtk"]).unwrap();
        assert!(cli.gtk);
        assert!(!cli.tui);
    }
}
