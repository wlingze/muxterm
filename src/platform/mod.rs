//! 平台适配层：所有前端。
//!
//! - `cli`：命令行前端（CLI 命令模式）
//! - `tui`：crossterm TUI 前端（feature = "tui"）
//! - `linux`：GTK4 原生前端（feature = "gtk"）
//! - `macos`：SwiftUI 前端（Swift 代码，不在 Rust 编译范围）；
//!   此模块提供从 Rust 侧 `muxterm gui` 定位并 `open` Muxterm.app 的启动器。

pub use crate::frontend::cli;
pub mod format;
pub mod i18n;
pub mod mirror;
pub mod mouse;
pub mod ssh_probe;
pub mod url_opener;

pub use crate::frontend::command_queue;
pub use crate::frontend::ffi_client;

#[cfg(any(feature = "gtk", feature = "tui"))]
pub use crate::frontend::event_pump;

#[cfg(target_os = "macos")]
pub use crate::frontend::macos;

#[cfg(feature = "gtk")]
pub use crate::frontend::linux;

#[cfg(feature = "tui")]
pub use crate::frontend::tui;
