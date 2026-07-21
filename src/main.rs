// Muxterm 主入口。
//
// 根据 Cargo feature flag + CLI flag 选择前端：
// - `gtk`：GTK4 原生前端
// - `tui`：纯 crossterm TUI 前端
// 至少要启用一个，否则编译期报错。同时启用两个时用 `--tui` / `--gtk` 选择。

use clap::Parser;

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
    // 编译期保证至少启用了一个前端 feature。
    #[cfg(not(any(feature = "gtk", feature = "tui")))]
    compile_error!("muxterm 需要至少启用一个前端 feature: `gtk` 或 `tui`");

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

    // 决定前端：显式 --tui 或 --gtk 优先；否则默认 GTK（启用时）→ TUI。
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

    #[test]
    fn cli_tui_and_gtk_mutually_exclusive_values() {
        // clap 允许同时传，但运行时 want_tui 优先；这里只验证解析不 panic。
        let cli = Cli::try_parse_from(["muxterm", "--tui", "--gtk"]).unwrap();
        assert!(cli.tui);
        assert!(cli.gtk);
    }
}
