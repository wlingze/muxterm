//! Frontend 层：cli / tui / linux / macos / windows，只经 FFI + `ffi_client` 用 Core。

#![allow(dead_code)]
#![allow(clippy::should_implement_trait)]

pub mod cli;
pub mod command_queue;
pub mod event_pump;
pub mod ffi_client;
pub mod format;
pub mod i18n;
#[cfg(feature = "gtk")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod mirror;
pub mod mouse;
pub mod ssh_probe;
#[cfg(feature = "tui")]
pub mod tui;
pub mod url_opener;
pub mod view_store;
