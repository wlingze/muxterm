//! UI 层（GTK4 + vte4）。
//!
//! 模块：
//! - `theme`：主题 → ANSI 样式映射（纯函数）
//! - `app`：GTK Application 启动
//! - `window`：主窗口（Notebook + 输入框 + 状态栏）
//! - `notebook`：tab 管理（每个 pane 一个 tab）
//! - `pane_view`：vte4 终端输出视图
//! - `input_bar`：底部输入框 + 快捷键
//! - `wiring`：tmux client ↔ UI 的事件桥接

pub mod app;
pub mod input_bar;
pub mod notebook;
pub mod pane_view;
pub mod theme;
pub mod window;
pub mod wiring;
