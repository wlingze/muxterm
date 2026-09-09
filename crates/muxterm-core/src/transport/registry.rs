//! Reusable target connections.

use super::TransportProvider;
pub use muxterm_transport::ConnectionRegistry;

/// Construct the built-in TransportProvider list in stable UI order.
pub fn with_builtins() -> Vec<Box<dyn TransportProvider>> {
    vec![
        Box::new(super::local::provider::LocalTransport),
        Box::new(super::ssh::provider::SshTransport),
    ]
}
