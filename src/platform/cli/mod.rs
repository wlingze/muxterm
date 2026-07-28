//! CLI 命令模块：命令解析 + 输出格式化。
//!
//! 不依赖任何 feature flag（gtk/tui），任何构建都能用 `muxterm <command>`。
//! 复用 TerminalModel + Backend 接口，不经过 UI 渲染层。

pub mod client;
pub mod command;
pub mod daemon;
pub mod format;
pub mod ipc;
pub mod session;
pub mod tmux_cli;
pub mod tmux_cli_exec;

pub use command::{parse_cli_command, CliCommand};
pub use format::{format_output, OutputFormat, StateSnapshot};
// pub use tmux_cli::{parse_tmux_cli, CliEnvelope, TmuxCliCommand};

pub mod entry;
