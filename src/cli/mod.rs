//! CLI 命令模块：命令解析 + 输出格式化。
//!
//! 不依赖任何 feature flag（gtk/tui），任何构建都能用 `muxterm <command>`。
//! 复用 TerminalModel + Backend 接口，不经过 UI 渲染层。

pub mod command;
pub mod format;

pub use command::{parse_cli_command, CliCommand, CliError};
pub use format::{format_output, OutputFormat};
