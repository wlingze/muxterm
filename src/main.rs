// Muxterm 主入口。
//
// 根据 Cargo feature flag 选择前端：
// - `gtk`：GTK4 原生前端
// - `tui`：纯 crossterm TUI 前端
// 至少要启用一个，否则编译期报错。

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

    // 根据启用的 feature 选择前端运行时。同时启用两个时优先 GTK。
    #[cfg(feature = "gtk")]
    {
        tracing::info!(target = "muxterm", "muxterm 启动（GTK4 UI）");
        platform::linux::app::run(cli.socket)
    }
    #[cfg(all(not(feature = "gtk"), feature = "tui"))]
    {
        tracing::info!(target = "muxterm", "muxterm 启动（TUI）");
        platform::tui::app::run(cli.socket)
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
    }
}
