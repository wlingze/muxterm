//! Linux app layer: GTK application, lifecycle, and the main window.

mod launch;
pub mod lifecycle;
pub mod window;
pub mod window_input;

pub use launch::*;
