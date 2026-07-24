//! TUI 前端（crossterm，经 FFI 调核心）。
//!
//! ASCII 文本终端前端：`CoreBridge` 拉快照 → `render` 画帧；键盘经
//! `execute` / `send_input` 回写。适合无 GTK 的机器（headless / SSH）。

pub mod app;
pub mod ffi_bridge;
pub mod render;
