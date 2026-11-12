//! Ports: the contracts infrastructure adapters must implement.
//!
//! Every trait here lives inside the core so that adapters depend on the domain
//! and never the inverse.

use async_trait::async_trait;

use crate::{
    domain::{
        Body, BodyDirection, CaptureEvent, CaptureSession, CaptureSource, ExchangeId,
        ExchangeQuery, ExchangeState, ExchangeSummary, ExchangeUpdate, NetworkExchange,
        NetworkResponse, SessionId, SessionSummary, Timestamp, TimingMetrics,
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

    /// Records which process is capturing a session.
    ///
    /// Recovery uses this to tell a crashed session from one another `netra`
    /// process is capturing right now.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::NotFound`] when the session is missing.
    async fn set_owner(&self, id: SessionId, pid: u32) -> StorageResult<()>;

    /// Marks every session abandoned by a dead process as stopped at `at`.
    ///
    /// A session is abandoned, never merely "active": when `owner_pid` names a
    /// live process the session is left untouched, because that process is
    /// still writing to it.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Backend`] on write failure.
    async fn recover_interrupted(
        &self,
        at: Timestamp,
        probe: &dyn ProcessProbe,
    ) -> StorageResult<Vec<SessionId>>;
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

    /// Applies one incremental update inside a transaction.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::NotFound`] when the target exchange
    /// disappeared, and [`crate::error::StorageError::Integrity`] when the parent
    /// session is missing.
    async fn apply(&self, update: &ExchangeUpdate) -> StorageResult<()>;

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

    /// Deletes the oldest exchanges of a session beyond `keep`.
    ///
    /// Implements the configured per-session retention so one long capture
    /// cannot grow the database without bound.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Backend`] on write failure.
    async fn enforce_retention(&self, session_id: SessionId, keep: u64) -> StorageResult<u64>;

    /// Distinct hosts observed in a session, with request counts.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::Backend`] on query failure.
    async fn hosts(&self, session_id: SessionId) -> StorageResult<Vec<(String, u64)>>;

    /// Rewrites one side of an exchange with a payload replacement.
    ///
    /// Used by late body retrieval: the response later reports its body, and
    /// the stored row is updated without rewriting headers.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::NotFound`] when the exchange is missing.
    async fn update_body(
        &self,
        id: ExchangeId,
        direction: BodyDirection,
        body: &Body,
    ) -> StorageResult<()>;

    /// Updates response header data after a late `responseReceived` event.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::NotFound`] when the exchange is missing.
    async fn update_response(
        &self,
        id: ExchangeId,
        response: &NetworkResponse,
    ) -> StorageResult<()>;

    /// Updates state, error text and timings.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::StorageError::NotFound`] when the exchange is missing.
    async fn update_state(
        &self,
        id: ExchangeId,
        state: ExchangeState,
        error: Option<&str>,
        timing: Option<TimingMetrics>,
    ) -> StorageResult<()>;
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

/// Answers whether a process is still running.
pub trait ProcessProbe: Send + Sync {
    /// `true` when the process exists.
    fn is_alive(&self, pid: u32) -> bool;
}

/// Production probe: on Linux a live process owns an entry in `/proc`.
///
/// On other platforms the probe reports every process as gone, which keeps the
/// recovery behaviour conservative (a session is recovered on the next start)
/// and is documented in `docs/architecture.md`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProcessProbe;

impl ProcessProbe for SystemProcessProbe {
    fn is_alive(&self, pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            std::path::Path::new(&format!("/proc/{pid}")).exists()
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            false
        }
    }
}

/// Probe that reports a fixed set of processes as alive, for tests.
#[derive(Debug, Clone, Default)]
pub struct FixedProcessProbe {
    alive: std::collections::BTreeSet<u32>,
}

impl FixedProcessProbe {
    /// Builds a probe from the pids that must look alive.
    #[must_use]
    pub fn new(alive: impl IntoIterator<Item = u32>) -> Self {
        Self {
            alive: alive.into_iter().collect(),
        }
    }
}

impl ProcessProbe for FixedProcessProbe {
    fn is_alive(&self, pid: u32) -> bool {
        self.alive.contains(&pid)
    }
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
