//! CLI command parsing, routing, and daemon frontend.

pub mod application;
pub mod config;
pub mod daemon;
pub mod format;
pub mod routing;
pub mod session;
pub mod tmux_cli;
pub mod tmux_cli_exec;

pub use crate::protocol::command::{parse_cli_command, CliCommand};
pub use format::{format_ffi_output, format_output, OutputFormat};
#[allow(unused_imports)]
pub use tmux_cli::{parse_tmux_cli, CliEnvelope, Target, TmuxCliCommand};
