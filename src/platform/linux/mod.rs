//! Linux 前端（GTK4 + vte4），经 FFI 调用核心。

pub mod app;
pub mod attention_ui;
pub mod command_palette;
pub mod ffi_bridge;
pub mod input_bar;
pub mod keymap;
pub mod layout_host;
pub mod lifecycle;
pub mod notebook;
pub mod pane_switcher;
pub mod pane_view;
pub mod panel_model;
pub mod preferences_window;
pub mod quick_pick;
pub mod quickconnect;
pub mod quickconnect_panel;
pub mod renderer;
pub mod scrollback_view;
pub mod status_bar;
pub mod tab_bar;
pub mod target_config_window;
pub mod theme;
pub mod title_watch;
pub mod tmux_dialog;
pub mod window;
