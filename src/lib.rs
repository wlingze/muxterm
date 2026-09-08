#![allow(dead_code)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::borrowed_box)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::needless_pass_by_value)]
//! Muxterm library root.

/// Stable application services used by the thin binary entry point.
pub mod app {
    pub use crate::core::fault::install_hook;
    pub use crate::core::logging::{init_logging, resolve_config, LoggingConfig};
}

/// Public C-ABI facade. Core implementation modules remain behind this boundary
/// for frontend callers that use the FFI contract.
#[cfg(feature = "ffi")]
pub mod ffi {
    pub use crate::core::protocol::ffi::*;
}

pub mod core;
pub mod platform;
