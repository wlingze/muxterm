//! Built-in RuntimeProvider registration.

use super::RuntimeProvider;

/// Runtime providers owned by the product composition root.
pub struct RuntimeRegistry {
    providers: Vec<Box<dyn RuntimeProvider>>,
}

impl Default for RuntimeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Construct the built-in RuntimeProvider registry in stable UI order.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(super::tmux::TmuxDriver));
        registry.register(Box::new(super::herdr::HerdrDriver));
        registry.register(Box::new(super::shell::ShellDriver));
        registry
    }

    /// Register a provider. Existing ids are replaced in place so UI order is
    /// stable; new ids are appended.
    pub fn register(&mut self, provider: Box<dyn RuntimeProvider>) {
        let id = provider.id();
        if let Some(index) = self.providers.iter().position(|item| item.id() == id) {
            self.providers[index] = provider;
        } else {
            self.providers.push(provider);
        }
    }

    pub fn get(&self, id: &str) -> Option<&dyn RuntimeProvider> {
        self.providers
            .iter()
            .find(|provider| provider.id() == id)
            .map(|provider| provider.as_ref())
    }

    pub fn providers(&self) -> &[Box<dyn RuntimeProvider>] {
        &self.providers
    }
}

/// Compatibility helper for callers that still assemble a Catalog directly.
pub fn with_builtins() -> Vec<Box<dyn RuntimeProvider>> {
    RuntimeRegistry::with_builtins().providers
}
