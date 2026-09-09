//! 平台适配层：所有前端。
//!
//! - `cli`：命令行前端（CLI 命令模式）
//! - `tui`：crossterm TUI 前端（feature = "tui"）
//! - `linux`：GTK4 原生前端（feature = "gtk"）
//! - `macos`：SwiftUI 前端（Swift 代码，不在 Rust 编译范围）；
//!   此模块提供从 Rust 侧 `muxterm gui` 定位并 `open` Muxterm.app 的启动器。

pub use crate::frontend::cli;
pub use crate::frontend::format;
pub use crate::frontend::i18n;
pub use crate::frontend::mirror;
pub use crate::frontend::mouse;
pub use crate::frontend::ssh_probe;
pub use crate::frontend::url_opener;

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
