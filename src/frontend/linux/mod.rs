//! Linux 前端（GTK4 + vte4），经 FFI 调用核心。
//!
//! 分层：[`app`]（启动/主窗）、[`chrome`]（骨架/侧栏/面板）、
//! [`terminal`]（PaneSurface）、[`ui`]（设置/连接对话框）。

pub mod app;
pub mod chrome;
pub mod terminal;
pub mod ui;

pub use crate::frontend::view_store;

pub use app::lifecycle;
pub use app::window;
pub use app::window_input;
pub use chrome::app_shell;
pub(crate) use chrome::attention_compat;
pub use chrome::attention_ui;
pub use chrome::command_palette;
pub use chrome::keymap;
pub use chrome::overlay;
pub use chrome::panel_model;
pub use chrome::scene_stack;
pub use chrome::status_bar;
pub use chrome::workspace_scenes;
pub use chrome::workspace_sidebar;
#[cfg(test)]
pub(crate) use terminal::event_batch;
pub use terminal::font_registry;
pub use terminal::input_bar;
pub use terminal::layout_host;
pub(crate) use terminal::pane_input_state;
pub use terminal::pane_switcher;
pub use terminal::pane_view;
pub use terminal::scroll_policy;
pub use terminal::scrollback_view;
pub use ui::fault_gtk;
pub use ui::preferences_window;
pub use ui::quick_pick;
pub use ui::quickconnect;
pub use ui::quickconnect_panel;
pub use ui::settings_model;
pub use ui::target_config_window;
pub use ui::theme;
pub use ui::tmux_dialog;
