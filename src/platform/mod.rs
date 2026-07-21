//! 平台适配层。
//!
//! 用 Cargo feature flag 选择前端：
//! - `gtk`：GTK4 原生前端（`linux` 模块）
//! - `tui`：纯 crossterm TUI 前端（`tui` 模块）
//!
//! 至少要启用其中一个，否则 `main` 在启动时报错。

#[cfg(feature = "gtk")]
pub mod linux;

#[cfg(feature = "tui")]
pub mod tui;
