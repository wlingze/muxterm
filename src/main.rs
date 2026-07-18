// Muxterm 主入口。
//
// Phase 2：GTK4 终端 UI，连本地 tmux -CC，把 pane 渲染成 tab。

use clap::Parser;

mod config;
mod tmux;
mod ui;


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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    tracing::info!(target = "muxterm", "muxterm 启动（GTK4 UI phase）");
    ui::app::run()
}
