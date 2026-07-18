// Muxterm 主入口
//
// 当前 phase 只实现 tmux 控制协议客户端库，UI 留待后续 phase。
// 这里只做最小占位：初始化日志并打印欢迎信息。

use clap::Parser;

/// Muxterm 命令行参数
#[derive(Parser, Debug)]
#[command(name = "muxterm", version, about = "Native UI terminal for tmux control mode")]
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

    tracing::info!("muxterm 启动（协议库 phase，UI 待后续 phase）");
    println!("欢迎使用 Muxterm —— tmux 控制模式客户端库已就绪。");
    println!("当前 phase: 协议解析 / 命令构造 / 异步客户端。UI 留待后续 phase。");
    Ok(())
}
