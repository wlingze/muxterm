//! Concrete transport implementations built on the shared transport contract.

pub mod connection;
pub mod local;
pub use muxterm_transport::provider;
pub mod registry;
pub mod ssh;

pub use muxterm_transport::{
    ByteChannel, ChannelKind, ChannelRequest, CommandOutput, PtySize, TargetConnection,
    TrafficCounters, Transport, TransportError, TransportSignal,
};
