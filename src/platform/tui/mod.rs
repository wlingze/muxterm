//! TUI 前端（crossterm，无 GTK 依赖）。
//!
//! ASCII 文本终端前端：渲染 `State` 快照 + 把键盘事件转成 `Task` 发给
//! `TerminalModel`。适合无 GTK 的机器（headless / SSH）跑 muxterm。

pub mod app;
pub mod render;
