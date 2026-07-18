//! UI 层（GTK4 + vte4）。
//!
//! 模块：
//! - `theme`：主题 → ANSI 样式映射（纯函数）
//! - `app`：GTK Application 启动
//! - `window`：主窗口（工具栏 + Notebook + 输入栏 + 状态栏）
//! - `notebook`：tab 管理（本地 shell / tmux pane）
//! - `pane_view`：vte4 终端视图（本地 shell 自 spawn 子进程 / tmux pane feed 输出）
//! - `input_bar`：底部输入框 + 快捷键（仅 tmux pane 显示）
//! - `tmux_dialog`：tmux 集成对话框（列 session / attach / 新建 session）
//! - `wiring`：tmux client ↔ UI 事件桥接

pub mod app;
pub mod input_bar;
pub mod notebook;
pub mod pane_view;
pub mod theme;
pub mod tmux_dialog;
pub mod window;
pub mod wiring;
