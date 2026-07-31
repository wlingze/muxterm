#![allow(unreachable_code)]
#![allow(dead_code)]
// Muxterm 主入口（thin entry）。
//
// CLI 命令模式：`muxterm <command> [options]` → platform::cli::routing
// 交互模式：`muxterm --tui` / `muxterm --gtk` → platform::{tui,linux}

use clap::Parser;

mod core;
mod platform;

/// Muxterm 命令行参数（交互模式用）
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

    /// 使用 TUI 前端
    #[arg(long = "tui", default_value_t = false)]
    tui: bool,

    /// 使用 GTK4 前端
    #[arg(long = "gtk", default_value_t = false)]
    gtk: bool,
}

fn main() -> anyhow::Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();

    // CLI 命令模式：第一个参数不以 - 开头
    if let Some(first) = raw_args.get(1) {
        if !first.starts_with('-') && !first.is_empty() {
            return platform::cli::routing::run_cli(&raw_args[1..]);
        }
    }

    // 交互模式
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

    let want_tui = cli.tui || (!cli.gtk && cfg!(not(feature = "gtk")) && cfg!(feature = "tui"));
    let want_gtk = !want_tui && cfg!(feature = "gtk");

    if want_tui {
        #[cfg(feature = "tui")]
        {
            tracing::info!(target = "muxterm", "muxterm 启动（TUI）");
            if let Some(ref name) = cli.session {
                if cli.socket.is_none() {
                    platform::cli::routing::ensure_local_daemon(name)?;
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
