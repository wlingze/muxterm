//! Reusable target-level connections.

use std::collections::HashMap;
use std::sync::Arc;

use crate::transport::{TargetConnection, TransportResult};

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

    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
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
    ) -> TransportResult<Arc<dyn TargetConnection>>
    where
        F: FnOnce() -> TransportResult<Arc<dyn TargetConnection>>,
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
    use crate::transport::{ByteChannel, ChannelRequest, CommandOutput};

    struct MockConnection {
        transport_id: &'static str,
        target: &'static str,
    }

    impl TargetConnection for MockConnection {
        fn transport_id(&self) -> &str {
            self.transport_id
        }

        fn target(&self) -> &str {
            self.target
        }

        fn open_channel(&self, _request: ChannelRequest) -> TransportResult<Box<dyn ByteChannel>> {
            Err(crate::transport::TransportError::message(
                "channel not used by registry test",
            ))
        }

        fn exec_command(&self, _request: ChannelRequest) -> TransportResult<CommandOutput> {
            Err(crate::transport::TransportError::message(
                "command not used by registry test",
            ))
        }

        fn probe(&self) -> TransportResult<()> {
            Ok(())
        }
    }

    #[test]
    fn acquire_reuses_one_target_connection() {
        let mut registry = ConnectionRegistry::new();
        let first = registry
            .acquire("local", "", || {
                Ok(Arc::new(MockConnection {
                    transport_id: "local",
                    target: "",
                }) as Arc<dyn TargetConnection>)
            })
            .unwrap();
        let second = registry
            .acquire("local", "", || {
                panic!("the factory must not run for an existing target")
            })
            .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn remove_releases_only_the_requested_target() {
        let mut registry = ConnectionRegistry::new();
        registry
            .acquire("local", "", || {
                Ok(Arc::new(MockConnection {
                    transport_id: "local",
                    target: "",
                }) as Arc<dyn TargetConnection>)
            })
            .unwrap();
        registry
            .acquire("ssh", "dev", || {
                Ok(Arc::new(MockConnection {
                    transport_id: "ssh",
                    target: "dev",
                }) as Arc<dyn TargetConnection>)
            })
            .unwrap();

        assert!(registry.remove("local", "").is_some());
        assert!(registry.get("local", "").is_none());
        assert!(registry.get("ssh", "dev").is_some());
        assert_eq!(registry.len(), 1);
    }
}
