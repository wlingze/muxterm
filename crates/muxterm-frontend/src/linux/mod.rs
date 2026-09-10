//! Linux 前端（GTK4 + vte4），经 FFI 调用核心。

pub mod app;
pub mod app_shell;
pub(crate) mod attention_compat;
pub mod attention_ui;
pub mod command_palette;
#[cfg(test)]
pub(crate) mod event_batch;
pub mod fault_gtk;
pub mod font_registry;
pub mod input_bar;
pub mod keymap;
pub mod layout_host;
pub mod lifecycle;
pub mod overlay;
pub(crate) mod pane_input_state;
pub mod pane_switcher;
pub mod pane_view;
pub mod panel_model;
pub mod preferences_window;
pub mod quick_pick;
pub mod quickconnect;
pub mod quickconnect_panel;
pub mod scene_stack;
pub mod scroll_policy;
pub mod scrollback_view;
pub mod settings_model;
pub mod status_bar;
pub mod target_config_window;
pub mod theme;
pub mod tmux_dialog;
pub use crate::view_store;
pub mod window;
pub mod window_input;
pub mod workspace_scenes;
pub mod workspace_sidebar;
