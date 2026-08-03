#![allow(unreachable_code)]
#![allow(dead_code)]
// Muxterm 主入口（thin entry）。
//
// 子命令：
//   `muxterm <command> [options]` → CLI 命令模式（platform::cli::routing）
//   `muxterm gui [...]`            → GUI 前端（Linux=GTK4，macOS=Swift .app）
//   `muxterm tui [...]`            → TUI 前端（crossterm）
// 向后兼容 flag：`muxterm --tui` / `muxterm --gtk` 仍可用。

use clap::Parser;

mod core;
mod platform;

/// Muxterm 命令行参数（交互模式用）。
#[derive(Parser, Debug)]
#[command(
    name = "muxterm",
    version,
    about = "Native UI terminal for tmux control mode"
)]
struct Cli {
    #[arg(short, long)]
    verbose: bool,

    /// tmux socket 名（`-L`）
    #[arg(short = 'L', long = "socket", value_name = "SOCKET")]
    socket: Option<String>,

    /// session 名（`-s`）
    #[arg(short = 's', long = "session", value_name = "NAME")]
    session: Option<String>,

    /// 使用 TUI 前端（向后兼容 flag）
    #[arg(long = "tui", default_value_t = false)]
    tui: bool,

    /// 使用 GTK4 前端（向后兼容 flag）
    #[arg(long = "gtk", default_value_t = false)]
    gtk: bool,
}

fn main() -> anyhow::Result<()> {
    let mut raw: Vec<String> = std::env::args().collect();
    if !raw.is_empty() {
        raw.remove(0); // 程序名
    }

    // 子命令：muxterm gui / muxterm tui
    if let Some(sub) = raw.first() {
        match sub.as_str() {
            "gui" => return run_gui(&raw[1..]),
            "tui" => return run_tui(&raw[1..]),
            _ => {}
        }
    }

    // CLI 命令模式：第一个参数不以 `-` 开头（含 `tmux`、`new-session`、`split-pane` 等）
    if let Some(first) = raw.first() {
        if !first.starts_with('-') && !first.is_empty() {
            return platform::cli::routing::run_cli(&raw[..]);
        }
    }

    // 交互模式（向后兼容 `--tui` / `--gtk` flag）
    let cli = Cli::parse_from(std::iter::once("muxterm".to_string()).chain(raw.iter().cloned()));
    init_tracing(cli.verbose);
    log_socket(&cli);

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

/// `muxterm gui [...]`：启动 GUI 前端。
fn run_gui(args: &[String]) -> anyhow::Result<()> {
    let cli = Cli::parse_from(std::iter::once("muxterm".to_string()).chain(args.iter().cloned()));
    init_tracing(cli.verbose);
    log_socket(&cli);
    run_gui_inner(cli.socket, cli.session)
}

/// `muxterm tui [...]`：启动 TUI 前端。
fn run_tui(args: &[String]) -> anyhow::Result<()> {
    let cli = Cli::parse_from(std::iter::once("muxterm".to_string()).chain(args.iter().cloned()));
    init_tracing(cli.verbose);
    log_socket(&cli);
    run_tui_inner(cli.socket, cli.session)
}

fn init_tracing(verbose: bool) {
    let filter = if verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn log_socket(cli: &Cli) {
    if let Some(ref sock) = cli.socket {
        tracing::info!(target = "muxterm", socket = %sock, "使用独立 tmux socket (-L)");
    }
}

#[cfg_attr(not(feature = "gtk"), allow(unused_variables))]
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
}
