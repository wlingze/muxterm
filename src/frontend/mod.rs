//! Frontend-common ownership and transport helpers.
//!
//! Platform frontends stay under `platform` during the incremental migration,
//! while shared FFI ownership and command queuing live here.

pub mod cli;
pub mod command_queue;
pub mod ffi_client;

#[cfg(feature = "tui")]
pub mod tui;

#[cfg(feature = "gtk")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;
