//! TUI 启动入口（骨架）。
//!
//! 真正的 ASCII/crossterm 渲染在 Step 5 实现；当前仅提供一个可调用的
//! `run`，让 `main` 在只启用 `tui` feature 时能编译并启动。

/// 启动 TUI 前端。
///
/// `socket` 对应 CLI `-L/--socket`：非空时 tmux 调用统一带 `-L`。
pub fn run(socket: Option<String>) -> anyhow::Result<()> {
    // Step 5 占位：先只记录参数，不进入真正的事件循环。
    tracing::info!(
        target = "muxterm::tui",
        ?socket,
        "TUI 前端尚未实现（Step 5）"
    );
    Ok(())
}
