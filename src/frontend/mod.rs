//! Frontend-common ownership and transport helpers.
//!
//! Platform frontends stay under `platform` during the incremental migration,
//! while shared FFI ownership and command queuing live here.

pub mod cli;
pub mod command_queue;
pub mod ffi_client;
