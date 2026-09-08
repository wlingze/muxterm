//! Compatibility exports for the pre-Phase-3 Catalog paths.
//!
//! Runtime provider ownership lives in `core::runtime::provider`; discovery
//! DTO ownership lives in `core::protocol::candidate`.

pub use crate::core::protocol::candidate::ExistingCandidate as SessionCandidate;
pub use crate::core::runtime::provider::{RuntimeInfo, RuntimeProvider};

/// Deprecated compatibility name. New code uses [`RuntimeProvider`].
pub use RuntimeProvider as RuntimeDriver;
