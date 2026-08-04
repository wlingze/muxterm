//! TUI 前端（crossterm + ratatui，经 FFI 调核心）。
//!
//! 现代跨平台文本终端前端：`CoreBridge` 拉快照 → ratatui 渲染；键盘经
//! `execute` / `send_input` 回写。适合无 GTK 的机器（headless / SSH / Windows）。

pub mod app;
pub mod ffi_bridge;
pub mod palette;
pub mod render;
pub mod terminal;
pub mod theme;
