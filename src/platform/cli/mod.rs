//! CLI 命令模块：命令解析 + 输出格式化。
//!
//! 不依赖任何 feature flag（gtk/tui），任何构建都能用 `muxterm <command>`。
//! 复用 TerminalModel + Runtime 接口，不经过 UI 渲染层。

pub mod config;
pub mod daemon;
pub mod format;
pub mod session;
pub mod tmux_cli;
pub mod tmux_cli_exec;

pub use format::{format_ffi_output, format_output, OutputFormat, StateSnapshot};
pub use muxterm_protocol::command::{parse_cli_command, CliCommand};
#[allow(unused_imports)]
pub use tmux_cli::{parse_tmux_cli, CliEnvelope, Target, TmuxCliCommand};

pub mod application;
pub mod routing;
