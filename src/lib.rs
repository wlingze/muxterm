#![allow(dead_code)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::borrowed_box)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::needless_pass_by_value)]
//! Muxterm library root.

/// Public C-ABI facade. Core implementation modules remain behind this boundary
/// for frontend callers that use the FFI contract.
pub mod ffi {
    pub use muxterm_core::protocol::ffi::*;
}

pub mod frontend;

/// Test-only compatibility exports for the existing integration contract suite.
///
/// Product frontends use `ffi` and `frontend`; this namespace is intentionally
/// separate so the private Core/frontend module trees are not part of the
/// product API while the contract tests migrate.
#[doc(hidden)]
pub mod test_support {
    pub mod core {
        pub use muxterm_core::*;
    }

    pub mod platform {
        pub use crate::frontend::*;
    }
}
