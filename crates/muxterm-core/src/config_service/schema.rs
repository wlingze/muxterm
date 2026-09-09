//! Compatibility re-exports for the former `config_service::schema` module.
//!
//! Persistent document ownership lives in [`crate::config::document`]. New code
//! should import these types from [`crate::config`].

pub use crate::config::document::*;
