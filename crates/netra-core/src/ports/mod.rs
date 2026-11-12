//! Ports: the contracts infrastructure adapters must implement.
//!
//! Every trait here lives inside the core so that adapters depend on the domain
//! and never the inverse.

use async_trait::async_trait;

use crate::{
    domain::{
        CaptureEvent, CaptureSession, CaptureSource, ExchangeId, ExchangeQuery, ExchangeSummary,
        NetworkExchange, SessionId, SessionSummary, Timestamp,
    },
    error::{CaptureError, StorageResult},
};

/// A source of normalized capture events (CDP, proxy, ...).
#[async_trait]
pub trait CaptureProvider: Send + Sync {
    /// Which source this provider represents.
    fn source(&self) -> CaptureSource;

    /// Begins capturing.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::AlreadyRunning`] when called twice, or
    /// [`CaptureError::Source`] when the underlying transport fails.
    async fn start(&mut self) -> Result<(), CaptureError>;

    /// Stops capturing and releases resources. Must be idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Source`] when shutdown itself fails.
    async fn stop(&mut self) -> Result<(), CaptureError>;

    /// Waits for the next event.
    ///
    /// Returns `Ok(None)` when the provider is stopped and will emit nothing
    /// more. Errors are recoverable only if a later call can still succeed.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::StreamClosed`] when the provider failed
    /// irrecoverably and [`CaptureError::Source`] for transient failures.
    async fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError>;
}

/// Persistence of session aggregates.
#[async_trait]
pub trait SessionRepository: Send + Sync {
    /// Inserts a new session.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Integrity`] if the identifier exists.
    async fn create(&self, session: &CaptureSession) -> StorageResult<()>;

    /// Updates name, status and timestamps of an existing session.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::NotFound`] when the session is missing.
    async fn update(&self, session: &CaptureSession) -> StorageResult<()>;

    /// Loads one session.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Backend`] on query failure.
    async fn get(&self, id: SessionId) -> StorageResult<Option<CaptureSession>>;

    /// Lists sessions, newest first, with derived counters.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Backend`] on query failure.
    async fn list(&self) -> StorageResult<Vec<SessionSummary>>;

    /// Deletes a session and, by cascade, its exchanges.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::NotFound`] when the session is missing.
    async fn delete(&self, id: SessionId) -> StorageResult<()>;

    /// Marks every still-active session as stopped at `at`.
    ///
    /// Called once during startup: a session left in `capturing`/`paused` can
    /// only mean the previous process ended without a graceful stop.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Backend`] on write failure.
    async fn recover_interrupted(&self, at: Timestamp) -> StorageResult<Vec<SessionId>>;
}

/// Persistence of exchanges, written incrementally while capture runs.
#[async_trait]
pub trait ExchangeRepository: Send + Sync {
    /// Inserts or replaces one exchange with its headers and bodies.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Integrity`] when the parent session
    /// is missing.
    async fn upsert(&self, exchange: &NetworkExchange) -> StorageResult<()>;

    /// Loads a full exchange, including headers and stored bodies.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Backend`] on query failure.
    async fn get(&self, id: ExchangeId) -> StorageResult<Option<NetworkExchange>>;

    /// Returns one page of summaries matching the query.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Backend`] on query failure.
    async fn page(&self, query: &ExchangeQuery) -> StorageResult<Vec<ExchangeSummary>>;

    /// Counts exchanges of a session by state.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Backend`] on query failure.
    async fn counts(&self, session_id: SessionId) -> StorageResult<ExchangeCounts>;
}

/// Aggregated exchange counters for a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExchangeCounts {
    /// All persisted exchanges.
    pub total: u64,
    /// Failed plus aborted exchanges.
    pub errors: u64,
    /// Exchanges still waiting for a response.
    pub pending: u64,
}

/// Wall-clock abstraction so tests are deterministic.
pub trait Clock: Send + Sync {
    /// Current instant.
    fn now(&self) -> Timestamp;
}

/// Production clock backed by the system time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_advances_monotonically() {
        let clock = SystemClock;
        let first = clock.now();
        let second = clock.now();
        assert!(second >= first);
    }
}
