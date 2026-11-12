//! Error types shared by the domain and its ports.

use thiserror::Error;

/// Errors produced while building or mutating domain values.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// An HTTP method was not a valid RFC 9110 token.
    #[error("invalid HTTP method: {0}")]
    InvalidMethod(String),
    /// A status code was outside the three-digit HTTP range.
    #[error("invalid HTTP status code: {code}")]
    InvalidStatusCode {
        /// The rejected numeric value.
        code: u16,
    },
    /// A textual identifier could not be parsed.
    #[error("invalid identifier: {0}")]
    InvalidId(String),
    /// A session name was empty or too long after trimming.
    #[error("invalid session name: {reason}")]
    InvalidSessionName {
        /// Why the name was rejected.
        reason: String,
    },
    /// A duration or size overflowed its representation.
    #[error("numeric value out of range: {0}")]
    OutOfRange(String),
}

/// Errors raised by the redaction policy.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RedactionError {
    /// A header name was empty.
    #[error("header name must not be empty")]
    EmptyHeaderName,
    /// A query parameter or JSON field matcher was empty.
    #[error("redaction rule must not be empty")]
    EmptyRule,
}

/// Errors surfaced by capture providers.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// The underlying source (CDP target, proxy listener, ...) failed.
    #[error("capture source error: {0}")]
    Source(String),
    /// An operation required a running capture.
    #[error("capture source is not running")]
    NotRunning,
    /// Capture is already running.
    #[error("capture source is already running")]
    AlreadyRunning,
    /// The provider event stream closed unexpectedly.
    #[error("capture event stream closed")]
    StreamClosed,
    /// The requested source is not available in this build or environment.
    #[error("capture source unavailable: {0}")]
    Unavailable(String),
}

/// Errors surfaced by persistence adapters.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StorageError {
    /// The backend rejected the operation.
    #[error("storage backend error: {0}")]
    Backend(String),
    /// The requested row or aggregate does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// A database invariant (foreign key, unique, check) was violated.
    #[error("data integrity error: {0}")]
    Integrity(String),
    /// A stored value could not be decoded into the domain model.
    #[error("stored value could not be decoded: {0}")]
    Decode(String),
}

/// Convenience alias for storage results.
pub type StorageResult<T> = Result<T, StorageError>;
