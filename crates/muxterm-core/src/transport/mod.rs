//! Concrete transport implementations built on the shared transport contract.

pub mod registry;

pub use muxterm_transport::provider::{TargetInfo, TransportInfo, TransportProvider};
pub use muxterm_transport::{local, ssh};

pub use muxterm_transport::{
    ByteChannel, ChannelKind, ChannelRequest, CommandOutput, Connect, PtySize, TargetConnection,
    TrafficCounters, Transport, TransportError, TransportSignal,
};
