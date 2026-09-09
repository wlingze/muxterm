//! Compatibility re-exports for the extracted protocol ID crate.
//!
//! New code should import IDs from `muxterm_protocol`; this module remains
//! until the Core/runtime and integration fixtures finish their path migration.

pub use muxterm_protocol::{PaneId, TabId};
