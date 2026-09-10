//! Shared frontend-side clients and view-facing adapters.
//!
//! This crate is intentionally introduced before moving platform widgets out
//! of the root package.  It gives the common FFI client a compiler-enforced
//! dependency on Core's public ABI instead of the root module tree.

pub mod ffi_client;
