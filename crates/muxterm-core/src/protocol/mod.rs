//! Core protocol layer: model + terminal + ffi (C ABI).

pub use muxterm_protocol::error::{self, ProtocolError};
pub use muxterm_protocol::{candidate, layout, state, task, PaneId, TabId, WorkspaceId};
pub mod command;
pub mod terminal;

#[cfg(feature = "ffi")]
pub mod ffi;
