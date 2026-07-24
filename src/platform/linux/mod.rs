//! Linux 前端（GTK4 + vte4），经 FFI 调用核心。

pub mod app;
pub mod command_palette;
pub mod ffi_bridge;
pub mod input_bar;
pub mod keymap;
pub mod layout_host;
pub mod lifecycle;
pub mod notebook;
pub mod pane_switcher;
pub mod pane_view;
pub mod quick_pick;
pub mod renderer;
pub mod tab_bar;
pub mod theme;
pub mod title_watch;
pub mod tmux_dialog;
pub mod window;
