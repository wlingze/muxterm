#![allow(unreachable_code)]
#![allow(dead_code)]
// Muxterm main entry point (thin entry).
//
// Subcommand model:
//   `muxterm <cli-command> [...]` → CLI command mode (new-session / split-pane / list-tabs ...)
//   `muxterm gui [...]`           → GUI frontend (Linux=GTK4, macOS=Swift .app)
//   `muxterm tui [...]`           → TUI frontend (crossterm)
//   `muxterm tmux ...`            → tmux structured CLI (session/tab/pane)
// Backward-compatible flags: `muxterm --tui` / `muxterm --gtk` remain available.
//
// Every CLI command is a clap subcommand; `muxterm --help` lists the full set,
// and each subcommand prints its own usage via `muxterm <cmd> --help`.

use clap::{Parser, Subcommand};

mod core;
mod platform;

/// Muxterm 顶层参数（全局 flag + 子命令）。
#[derive(Parser, Debug)]
#[command(
    name = "muxterm",
    version,
    about = "Native UI terminal for tmux control mode"
)]
struct Cli {
    #[arg(short, long)]
    verbose: bool,

    /// tmux socket name (`-L`)
    #[arg(short = 'L', long = "socket", value_name = "SOCKET")]
    socket: Option<String>,

    /// session name (`-s`)
    #[arg(short = 's', long = "session", value_name = "NAME")]
    session: Option<String>,

    /// Use the TUI frontend (backward-compatible flag)
    #[arg(long = "tui", default_value_t = false)]
    tui: bool,

    /// Use the GTK4 frontend (backward-compatible flag)
    #[arg(long = "gtk", default_value_t = false)]
    gtk: bool,

    #[command(subcommand)]
    cmd: Option<CliSubcommand>,
}

/// All subcommands.
///
/// CLI commands pass their raw arguments through to
/// `platform::cli::routing::run_cli` via `trailing_var_arg` +
/// `allow_hyphen_values` (reusing the existing hand-written parser), so short
/// flags such as `split-pane -h` (horizontal) are not swallowed by clap.
/// `disable_help_flag` keeps `-h` from conflicting with help.
#[derive(Subcommand, Debug)]
enum CliSubcommand {
    /// Create a new session
    #[command(alias = "new", disable_help_flag = true)]
    NewSession {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Kill a session
    #[command(disable_help_flag = true)]
    KillSession {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List sessions
    #[command(alias = "ls", disable_help_flag = true)]
    ListSessions {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Attach to a session
    #[command(alias = "attach", disable_help_flag = true)]
    AttachSession {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Detach from the current session
    #[command(disable_help_flag = true)]
    Detach {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Rename a session
    #[command(disable_help_flag = true)]
    RenameSession {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Create a new window
    #[command(alias = "neww", disable_help_flag = true)]
    NewWindow {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Kill a window
    #[command(alias = "killw", disable_help_flag = true)]
    KillWindow {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List windows
    #[command(alias = "lsw", disable_help_flag = true)]
    ListWindows {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Select a window
    #[command(alias = "selectw", disable_help_flag = true)]
    SelectWindow {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Rename a window
    #[command(alias = "renamew", disable_help_flag = true)]
    RenameWindow {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Create a new tab
    #[command(disable_help_flag = true)]
    NewTab {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Kill a tab
    #[command(disable_help_flag = true)]
    KillTab {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List tabs
    #[command(alias = "lst", disable_help_flag = true)]
    ListTabs {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Select a tab
    #[command(disable_help_flag = true)]
    SelectTab {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Rename a tab
    #[command(disable_help_flag = true)]
    RenameTab {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Split a pane (`-h` horizontal / `-v` vertical)
    #[command(alias = "splitp", disable_help_flag = true)]
    SplitPane {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Kill a pane
    #[command(alias = "killp", disable_help_flag = true)]
    KillPane {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List panes
    #[command(alias = "lsp", disable_help_flag = true)]
    ListPanes {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Select a pane
    #[command(alias = "selectp", disable_help_flag = true)]
    SelectPane {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Resize a pane
    #[command(alias = "resizep", disable_help_flag = true)]
    ResizePane {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Send keystrokes
    #[command(alias = "send", disable_help_flag = true)]
    SendKeys {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Write raw bytes
    #[command(disable_help_flag = true)]
    WriteRaw {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Capture the pane screen
    #[command(alias = "capturep", disable_help_flag = true)]
    CapturePane {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List the pane layout
    #[command(disable_help_flag = true)]
    ListLayout {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Display a message
    #[command(disable_help_flag = true)]
    DisplayMessage {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Dump a full state snapshot
    #[command(disable_help_flag = true)]
    DumpState {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// tmux structured CLI (session/tab/pane)
    #[command(disable_help_flag = true)]
    Tmux {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Launch the TUI frontend
    Tui {
        /// tmux socket name (`-L`)
        #[arg(short = 'L', long = "socket", value_name = "SOCKET")]
        socket: Option<String>,
        /// session name (`-s`)
        #[arg(short = 's', long = "session", value_name = "NAME")]
        session: Option<String>,
    },
    /// Launch the GUI frontend
    Gui {
        /// tmux socket name (`-L`)
        #[arg(short = 'L', long = "socket", value_name = "SOCKET")]
        socket: Option<String>,
        /// session name (`-s`)
        #[arg(short = 's', long = "session", value_name = "NAME")]
        session: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    log_socket(&cli);

    // 子命令分派
    if let Some(cmd) = &cli.cmd {
        return match cmd {
            // CLI 命令模式：把 canonical 命令名 + 原始参数交给既有 run_cli。
            CliSubcommand::NewSession { args } => dispatch_cli("new-session", args),
            CliSubcommand::KillSession { args } => dispatch_cli("kill-session", args),
            CliSubcommand::ListSessions { args } => dispatch_cli("list-sessions", args),
            CliSubcommand::AttachSession { args } => dispatch_cli("attach-session", args),
            CliSubcommand::Detach { args } => dispatch_cli("detach", args),
            CliSubcommand::RenameSession { args } => dispatch_cli("rename-session", args),
            CliSubcommand::NewWindow { args } => dispatch_cli("new-window", args),
            CliSubcommand::KillWindow { args } => dispatch_cli("kill-window", args),
            CliSubcommand::ListWindows { args } => dispatch_cli("list-windows", args),
            CliSubcommand::SelectWindow { args } => dispatch_cli("select-window", args),
            CliSubcommand::RenameWindow { args } => dispatch_cli("rename-window", args),
            CliSubcommand::NewTab { args } => dispatch_cli("new-tab", args),
            CliSubcommand::KillTab { args } => dispatch_cli("kill-tab", args),
            CliSubcommand::ListTabs { args } => dispatch_cli("list-tabs", args),
            CliSubcommand::SelectTab { args } => dispatch_cli("select-tab", args),
            CliSubcommand::RenameTab { args } => dispatch_cli("rename-tab", args),
            CliSubcommand::SplitPane { args } => dispatch_cli("split-pane", args),
            CliSubcommand::KillPane { args } => dispatch_cli("kill-pane", args),
            CliSubcommand::ListPanes { args } => dispatch_cli("list-panes", args),
            CliSubcommand::SelectPane { args } => dispatch_cli("select-pane", args),
            CliSubcommand::ResizePane { args } => dispatch_cli("resize-pane", args),
            CliSubcommand::SendKeys { args } => dispatch_cli("send-keys", args),
            CliSubcommand::WriteRaw { args } => dispatch_cli("write-raw", args),
            CliSubcommand::CapturePane { args } => dispatch_cli("capture-pane", args),
            CliSubcommand::ListLayout { args } => dispatch_cli("list-layout", args),
            CliSubcommand::DisplayMessage { args } => dispatch_cli("display-message", args),
            CliSubcommand::DumpState { args } => dispatch_cli("dump-state", args),
            CliSubcommand::Tmux { args } => dispatch_cli("tmux", args),
            CliSubcommand::Tui { socket, session } => {
                run_tui_inner(socket.clone(), session.clone())
            }
            CliSubcommand::Gui { socket, session } => {
                run_gui_inner(socket.clone(), session.clone())
            }
        };
    }

    // 无子命令：向后兼容 `--tui` / `--gtk` flag。
    let want_tui = cli.tui || (!cli.gtk && cfg!(not(feature = "gtk")) && cfg!(feature = "tui"));
    let want_gtk = !want_tui && cfg!(feature = "gtk");

    if want_tui {
        return run_tui_inner(cli.socket, cli.session);
    }
    if want_gtk {
        return run_gui_inner(cli.socket, cli.session);
    }
    anyhow::bail!("没有可用的前端（启用 `gtk` 或 `tui` feature）")
}

/// 组装 canonical 命令名 + 原始参数，交给既有 CLI 路由。
fn dispatch_cli(name: &str, args: &[String]) -> anyhow::Result<()> {
    let mut full = vec![name.to_string()];
    full.extend_from_slice(args);
    platform::cli::routing::run_cli(&full)
}

fn init_tracing(verbose: bool) {
    let filter = if verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        // 日志走 stderr，避免污染 stdout 上的 CLI JSON 输出。
        .with_writer(std::io::stderr)
        .init();
}

fn log_socket(cli: &Cli) {
    if let Some(ref sock) = cli.socket {
        tracing::info!(target = "muxterm", socket = %sock, "使用独立 tmux socket (-L)");
    }
}

#[allow(unused_variables)]
#[allow(clippy::needless_return)]
fn run_gui_inner(socket: Option<String>, session: Option<String>) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        return platform::macos::launch_app_bundle(socket.as_deref(), session.as_deref());
    }
    #[cfg(feature = "gtk")]
    {
        tracing::info!(target = "muxterm", "muxterm 启动（GTK4 UI）");
        platform::linux::app::run(socket)
    }
    #[cfg(all(not(target_os = "macos"), not(feature = "gtk")))]
    {
        anyhow::bail!("GUI 前端未编译（需要 --features gtk）")
    }
}

#[allow(unused_variables)]
fn run_tui_inner(socket: Option<String>, session: Option<String>) -> anyhow::Result<()> {
    #[cfg(feature = "tui")]
    {
        tracing::info!(target = "muxterm", "muxterm 启动（TUI）");
        if let Some(ref name) = session {
            if socket.is_none() {
                platform::cli::routing::ensure_local_daemon(name)?;
            }
        }
        platform::tui::app::run(platform::tui::app::TuiOpts { socket, session })
    }
    #[cfg(not(feature = "tui"))]
    {
        anyhow::bail!("TUI 前端未编译（需要 --features tui）")
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

    #[test]
    fn subcommand_new_session_captures_args() {
        let cli =
            Cli::try_parse_from(["muxterm", "new-session", "-n", "dev", "-s", "sock"]).unwrap();
        match cli.cmd {
            Some(CliSubcommand::NewSession { args }) => {
                assert_eq!(
                    args,
                    vec![
                        "-n".to_string(),
                        "dev".to_string(),
                        "-s".to_string(),
                        "sock".to_string()
                    ]
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn subcommand_split_pane_hyphen_passthrough() {
        // `-h` 是水平分屏，不能被 clap 当成帮助 flag 吞掉
        let cli = Cli::try_parse_from(["muxterm", "split-pane", "-h", "-t", "@1"]).unwrap();
        match cli.cmd {
            Some(CliSubcommand::SplitPane { args }) => {
                assert_eq!(
                    args,
                    vec!["-h".to_string(), "-t".to_string(), "@1".to_string()]
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn subcommand_tui_parses_socket_and_session() {
        let cli = Cli::try_parse_from(["muxterm", "tui", "-L", "test1", "-s", "demo"]).unwrap();
        match cli.cmd {
            Some(CliSubcommand::Tui { socket, session }) => {
                assert_eq!(socket.as_deref(), Some("test1"));
                assert_eq!(session.as_deref(), Some("demo"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn subcommand_gui_parses_socket() {
        let cli = Cli::try_parse_from(["muxterm", "gui", "-L", "sock"]).unwrap();
        match cli.cmd {
            Some(CliSubcommand::Gui { socket, .. }) => {
                assert_eq!(socket.as_deref(), Some("sock"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn subcommand_alias_new_maps_to_new_session() {
        let cli = Cli::try_parse_from(["muxterm", "new", "-n", "test"]).unwrap();
        assert!(matches!(cli.cmd, Some(CliSubcommand::NewSession { .. })));
    }

    #[test]
    fn subcommand_alias_ls_maps_to_list_sessions() {
        let cli = Cli::try_parse_from(["muxterm", "ls"]).unwrap();
        assert!(matches!(cli.cmd, Some(CliSubcommand::ListSessions { .. })));
    }
}
