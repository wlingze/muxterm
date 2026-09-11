//! TUI 前端（crossterm + ratatui，经 FFI 调核心）。
//!
//! Scene = per-workspace buffer 组。EventPump 写 ViewStore；渲染只读 Scene，
//! 切 workspace/tab 不向 Core 拉帧。

pub mod app;
pub mod emulate;
pub mod input;
pub mod model;
pub mod palette;
pub mod render;
pub mod scene;
pub mod terminal;
pub mod theme;
