//! Shared frontend-side clients and view-facing adapters.
//!
//! This crate is intentionally introduced before moving platform widgets out
//! of the root package.  It gives the common FFI client a compiler-enforced
//! dependency on Core's public ABI instead of the root module tree.

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
pub mod mirror;
pub mod mouse;
pub mod ssh_probe;
#[cfg(feature = "tui")]
pub mod tui;
pub mod url_opener;
pub mod view_store;
