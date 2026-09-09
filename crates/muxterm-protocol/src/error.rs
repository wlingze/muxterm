//! Stable product-level protocol errors.

/// Errors reported while validating or dispatching a product protocol action.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("不支持的 Task: {0}")]
    UnsupportedTask(String),
    #[error("muxterm ID 不存在: {0}")]
    IdNotFound(String),
    #[error("Runtime 未连接")]
    NotConnected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_remain_stable() {
        assert_eq!(ProtocolError::NotConnected.to_string(), "Runtime 未连接");
        assert_eq!(
            ProtocolError::IdNotFound("p1".into()).to_string(),
            "muxterm ID 不存在: p1"
        );
    }
}
