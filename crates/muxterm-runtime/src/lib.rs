//! Runtime contracts shared by Core and runtime provider implementations.

pub mod batch;
pub mod capability;
pub mod contract;
pub mod error;
pub mod provider;
pub mod tmux_protocol;

pub use batch::{ControlEvent, RenderEvent, RuntimeBatch, RuntimeSignal};
pub use capability::RuntimeCapability;
pub use contract::{Runtime, RuntimeSpec, WorktreeCreateSpec, WorktreeInfo};
pub use error::{RuntimeError, RuntimeResult};
pub use provider::{runtime_supports_channels, RuntimeInfo, RuntimeProvider};
