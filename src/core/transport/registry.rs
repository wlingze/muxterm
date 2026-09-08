//! Reusable target connections.

use std::collections::HashMap;
use std::sync::Arc;

use super::TargetConnection;

/// One reusable connection per `(transport_id, target)` identity.
#[derive(Default)]
pub struct ConnectionRegistry {
    connections: HashMap<(String, String), Arc<dyn TargetConnection>>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.connections.len()
    }

    pub fn get(&self, transport_id: &str, target: &str) -> Option<Arc<dyn TargetConnection>> {
        self.connections
            .get(&(transport_id.to_string(), target.to_string()))
            .cloned()
    }

    pub fn acquire<F>(
        &mut self,
        transport_id: &str,
        target: &str,
        connect: F,
    ) -> anyhow::Result<Arc<dyn TargetConnection>>
    where
        F: FnOnce() -> anyhow::Result<Arc<dyn TargetConnection>>,
    {
        let key = (transport_id.to_string(), target.to_string());
        if let Some(existing) = self.connections.get(&key) {
            return Ok(existing.clone());
        }
        let connection = connect()?;
        self.connections.insert(key, connection.clone());
        Ok(connection)
    }

    pub fn remove(
        &mut self,
        transport_id: &str,
        target: &str,
    ) -> Option<Arc<dyn TargetConnection>> {
        self.connections
            .remove(&(transport_id.to_string(), target.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::catalog::connect::Connect;

    #[test]
    fn acquire_reuses_one_target_connection() {
        let mut registry = ConnectionRegistry::new();
        let first = registry
            .acquire("local", "", || {
                Ok(Connect::new("local", "") as Arc<dyn TargetConnection>)
            })
            .unwrap();
        let second = registry
            .acquire("local", "", || {
                panic!("the factory must not run for an existing target")
            })
            .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(registry.len(), 1);
    }
}
