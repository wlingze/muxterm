//! Shared frontend-side clients and view-facing adapters.
//!
//! This crate is intentionally introduced before moving platform widgets out
//! of the root package.  It gives the common FFI client a compiler-enforced
//! dependency on Core's public ABI instead of the root module tree.

pub mod command_queue;
pub mod ffi_client;
pub mod format;
pub mod i18n;
pub mod mirror;
pub mod mouse;
pub mod ssh_probe;
pub mod url_opener;
