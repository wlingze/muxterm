//! TUI 前端（crossterm，无 GTK 依赖）。
//!
//! Step 5 才会实现真正的 TUI 渲染；这里先放一个最小骨架，
//! 让 `--no-default-features --features tui` 能编译链接到入口符号。

pub mod app;
