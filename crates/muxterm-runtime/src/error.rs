//! Errors exposed by the Runtime boundary.

use thiserror::Error;

/// Library-level error returned by Runtime instances and providers.
///
/// Runtime implementations still use `anyhow` internally while the provider
/// implementations are being split into their owning domains.  The public
/// trait boundary deliberately exposes this typed error so callers do not
/// depend on the application error container.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime operation is not supported: {operation}")]
    Unsupported { operation: &'static str },

    #[error("runtime operation failed: {message}")]
    Message { message: String },
}

impl RuntimeError {
    /// Convert an implementation-side error without leaking its container
    /// through the Runtime trait signature.
    pub fn message(error: impl std::fmt::Display) -> Self {
        Self::Message {
            message: error.to_string(),
        }
    }
}

impl From<anyhow::Error> for RuntimeError {
    fn from(error: anyhow::Error) -> Self {
        Self::message(format!("{error:#}"))
    }
}

/// Result alias for the Runtime library boundary.
pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;
