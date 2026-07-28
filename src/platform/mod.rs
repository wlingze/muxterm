//! 平台适配层：所有前端。
//!
//! - `cli`：命令行前端（CLI 命令模式）
//! - `tui`：crossterm TUI 前端（feature = "tui"）
//! - `linux`：GTK4 原生前端（feature = "gtk"）
//! - `macos`：SwiftUI 前端（Swift 代码，不在 Rust 编译范围）

pub mod cli;

#[cfg(feature = "gtk")]
pub mod linux;

#[cfg(feature = "tui")]
pub mod tui;
