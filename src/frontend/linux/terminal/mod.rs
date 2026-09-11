//! Linux terminal surfaces: pane widgets, layout host, scrollback.

#[cfg(test)]
pub(crate) mod event_batch;
pub mod font_registry;
pub mod input_bar;
pub mod layout_host;
pub(crate) mod pane_input_state;
pub mod pane_switcher;
pub mod pane_view;
pub mod scroll_policy;
pub mod scrollback_view;
