//! Application-level errors.

use netra_core::error::{CaptureError, DomainError, StorageError};
use thiserror::Error;

/// Failure of a use case.
#[derive(Debug, Error)]
pub enum ApplicationError {
    /// The domain rejected the input.
    #[error("{0}")]
    Domain(#[from] DomainError),
    /// Persistence failed.
    #[error("{0}")]
    Storage(#[from] StorageError),
    /// Capture failed.
    #[error("{0}")]
    Capture(#[from] CaptureError),
    /// The requested entity does not exist.
    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),
    /// The operation is not allowed in the current state.
    #[error("{0}")]
    Conflict(String),
    /// Encoding or decoding a file failed.
    #[error("{0}")]
    Format(String),
    /// The operation is not implemented yet.
    #[error("{feature} is not implemented yet")]
    Unsupported {
        /// Feature name.
        feature: &'static str,
    },
}

/// Convenience alias.
pub type ApplicationResult<T> = Result<T, ApplicationError>;
