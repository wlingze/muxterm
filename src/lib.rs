#![allow(dead_code)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::borrowed_box)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::needless_pass_by_value)]
//! Muxterm library root.

/// Stable application services used by the thin binary entry point.
pub mod app {
    /// Start the selected frontend from the single binary entry point.
    pub fn run() -> anyhow::Result<()> {
        crate::frontend::cli::application::run()
    }

    pub use crate::core::fault::install_hook;
    pub use crate::core::logging::{init_logging, resolve_config, LoggingConfig};

    /// Run a frontend callback behind the process-wide fault reporter.
    pub fn fault_run<T>(where_: &str, f: impl FnOnce() -> T) -> Option<T> {
        crate::core::fault::run(where_, f)
    }

    /// Return the most recent fault message for a frontend error dialog.
    pub fn last_fault_message() -> Option<String> {
        crate::core::fault::last_message()
    }

    /// Record a caught frontend fault before presenting its UI fallback.
    pub fn report_fault(where_: &str, payload: Box<dyn std::any::Any + Send>) {
        crate::core::fault::report(where_, payload)
    }
}

/// Public C-ABI facade. Core implementation modules remain behind this boundary
/// for frontend callers that use the FFI contract.
pub mod ffi {
    pub use crate::core::protocol::ffi::*;
}

pub(crate) use muxterm_core as core;
mod frontend;

/// Test-only compatibility exports for the existing integration contract suite.
///
/// Product frontends use `ffi` and `app`; this namespace is intentionally
/// separate so the private Core/frontend module trees are not part of the
/// product API while the contract tests migrate to their owning crates.
#[doc(hidden)]
pub mod test_support {
    pub mod core {
        pub use muxterm_core::*;
    }

    pub mod platform {
        pub use crate::frontend::*;
    }
}
