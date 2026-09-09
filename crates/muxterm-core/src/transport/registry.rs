//! Reusable target connections.

use super::TransportProvider;
pub use muxterm_transport::ConnectionRegistry;

/// Transport providers owned by the product composition root.
pub struct TransportRegistry {
    providers: Vec<Box<dyn TransportProvider>>,
}

impl Default for TransportRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TransportRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Construct the built-in TransportProvider registry in stable UI order.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(super::local::provider::LocalTransport));
        registry.register(Box::new(super::ssh::provider::SshTransport));
        registry
    }

    /// Register a provider. Existing ids are replaced in place so UI order is
    /// stable; new ids are appended.
    pub fn register(&mut self, provider: Box<dyn TransportProvider>) {
        let id = provider.id();
        if let Some(index) = self.providers.iter().position(|item| item.id() == id) {
            self.providers[index] = provider;
        } else {
            self.providers.push(provider);
        }
    }

    pub fn get(&self, id: &str) -> Option<&dyn TransportProvider> {
        self.providers
            .iter()
            .find(|provider| provider.id() == id)
            .map(|provider| provider.as_ref())
    }

    pub fn providers(&self) -> &[Box<dyn TransportProvider>] {
        &self.providers
    }
}

/// Compatibility helper for callers that still assemble a Catalog directly.
pub fn with_builtins() -> Vec<Box<dyn TransportProvider>> {
    TransportRegistry::with_builtins().providers
}
