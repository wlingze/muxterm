//! Frontend-common ownership and transport helpers.
//!
//! Platform frontends stay under `platform` during the incremental migration,
//! while shared FFI ownership and command queuing live here.

pub mod cli;
pub mod command_queue;
pub mod ffi_client;
pub mod format;
pub mod i18n;
pub mod mirror;
pub mod mouse;
pub mod ssh_probe;
pub mod url_opener;

#[cfg(any(feature = "gtk", feature = "tui"))]
pub mod event_pump;

#[cfg(feature = "tui")]
pub mod tui;

#[cfg(feature = "gtk")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;
