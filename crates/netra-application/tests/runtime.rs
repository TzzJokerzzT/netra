//! Capture runtime integration tests: persistence, pause, crash recovery and
//! storage-failure reporting.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use netra_application::{AppContext, AppEvent, CaptureRuntime, SessionService};
use netra_capture::demo::DemoCaptureProvider;
use netra_core::{
    domain::{
        Body, BodyDirection, CaptureSession, ExchangeId, ExchangeQuery, ExchangeState,
        ExchangeSummary, ExchangeUpdate, NetworkExchange, NetworkResponse, SessionId,
        SessionStatus, SourceSelection, TimingMetrics,
    },
    error::{StorageError, StorageResult},
    ports::{
        CaptureProvider, ExchangeCounts, ExchangeRepository, SystemClock,
    },
};
use netra_storage::SqliteStore;

fn context(store: Arc<SqliteStore>) -> AppContext {
    AppContext {
        sessions: store.clone(),
        exchanges: store,
        clock: Arc::new(SystemClock),
        config: netra_core::config::NetraConfig::with_default_data_dir(),
    }
}

async fn wait_for_total(
    store: &SqliteStore,
    session_id: SessionId,
    minimum: u64,
    timeout: Duration,
) -> u64 {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let counts = store.counts(session_id).await.unwrap();
        if counts.total >= minimum {
            return counts.total;
        }
        if tokio::time::Instant::now() >= deadline {
            return counts.total;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn demo_capture_is_persisted_incrementally() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let context = context(store.clone());
    let sessions = SessionService::new(context.clone());
    let session = sessions.create("demo", SourceSelection::Cdp).await.unwrap();

    let runtime = CaptureRuntime::start(
        context.clone(),
        session.clone(),
        vec![Box::new(DemoCaptureProvider::new())],
    )
    .await
    .unwrap();

    let total = wait_for_total(&store, session.id, 5, Duration::from_secs(5)).await;
    assert_eq!(total, 5, "every scripted exchange must be persisted");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut counts = store.counts(session.id).await.unwrap();
    while counts.errors == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
        counts = store.counts(session.id).await.unwrap();
    }
    assert!(counts.errors >= 1, "the failing exchange must be counted");
    assert!(counts.pending >= 1, "the redirect target stays pending");

    let rows = store
        .page(&ExchangeQuery::newest(session.id, 50))
        .await
        .unwrap();
    assert!(rows.iter().any(|row| row.state == ExchangeState::Completed));
    assert!(rows.iter().any(|row| row.method.as_str() == "POST"));

    runtime.stop().await.unwrap();
    let after = sessions.get(session.id).await.unwrap();
    assert_eq!(after.status, SessionStatus::Stopped);
    assert!(after.finished_at.is_some());
}

#[tokio::test]
async fn dropping_the_runtime_leaves_the_session_recoverable() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let context = context(store.clone());
    let sessions = SessionService::new(context.clone());
    let session = sessions
        .create("crashed", SourceSelection::Cdp)
        .await
        .unwrap();
    let runtime = CaptureRuntime::start(
        context.clone(),
        session.clone(),
        vec![Box::new(DemoCaptureProvider::new())],
    )
    .await
    .unwrap();
    assert!(wait_for_total(&store, session.id, 1, Duration::from_secs(5)).await >= 1);

    drop(runtime);

    assert_eq!(
        sessions.get(session.id).await.unwrap().status,
        SessionStatus::Capturing,
        "an aborted runtime must not look gracefully stopped"
    );
    let recovered = sessions.recover_interrupted().await.unwrap();
    assert_eq!(recovered, vec![session.id]);
    assert_eq!(
        sessions.get(session.id).await.unwrap().status,
        SessionStatus::Stopped
    );
}

#[tokio::test]
async fn pause_stops_recording_and_resume_continues() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let context = context(store.clone());
    let sessions = SessionService::new(context.clone());
    let session = sessions
        .create("pausable", SourceSelection::Cdp)
        .await
        .unwrap();
    let runtime = CaptureRuntime::start(
        context.clone(),
        session.clone(),
        vec![Box::new(DemoCaptureProvider::new())],
    )
    .await
    .unwrap();

    runtime.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;
    let paused_total = store.counts(session.id).await.unwrap().total;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        store.counts(session.id).await.unwrap().total,
        paused_total,
        "no exchange may be written while paused"
    );
    assert_eq!(
        sessions.get(session.id).await.unwrap().status,
        SessionStatus::Paused
    );

    runtime.resume().await.unwrap();
    let resumed_total =
        wait_for_total(&store, session.id, paused_total + 1, Duration::from_secs(5)).await;
    assert!(resumed_total > paused_total);
    runtime.stop().await.unwrap();
}

/// Repository that rejects every exchange write.
#[derive(Debug)]
struct FailingExchanges;

#[async_trait]
impl ExchangeRepository for FailingExchanges {
    async fn upsert(&self, _exchange: &NetworkExchange) -> StorageResult<()> {
        Err(StorageError::Backend("disk on fire".to_owned()))
    }

    async fn apply(&self, _update: &ExchangeUpdate) -> StorageResult<()> {
        Err(StorageError::Backend("disk on fire".to_owned()))
    }

    async fn get(&self, _id: ExchangeId) -> StorageResult<Option<NetworkExchange>> {
        Ok(None)
    }

    async fn page(&self, _query: &ExchangeQuery) -> StorageResult<Vec<ExchangeSummary>> {
        Ok(Vec::new())
    }

    async fn counts(&self, _session_id: SessionId) -> StorageResult<ExchangeCounts> {
        Ok(ExchangeCounts::default())
    }

    async fn enforce_retention(&self, _session_id: SessionId, _keep: u64) -> StorageResult<u64> {
        Ok(0)
    }

    async fn hosts(&self, _session_id: SessionId) -> StorageResult<Vec<(String, u64)>> {
        Ok(Vec::new())
    }

    async fn update_body(
        &self,
        _id: ExchangeId,
        _direction: BodyDirection,
        _body: &Body,
    ) -> StorageResult<()> {
        Err(StorageError::Backend("disk on fire".to_owned()))
    }

    async fn update_response(
        &self,
        _id: ExchangeId,
        _response: &NetworkResponse,
    ) -> StorageResult<()> {
        Err(StorageError::Backend("disk on fire".to_owned()))
    }

    async fn update_state(
        &self,
        _id: ExchangeId,
        _state: ExchangeState,
        _error: Option<&str>,
        _timing: Option<TimingMetrics>,
    ) -> StorageResult<()> {
        Err(StorageError::Backend("disk on fire".to_owned()))
    }
}

#[tokio::test]
async fn storage_failures_are_reported_never_swallowed() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let sessions = SessionService::new(context(store.clone()));
    let session = sessions
        .create("failing", SourceSelection::Cdp)
        .await
        .unwrap();
    let context = AppContext {
        sessions: store,
        exchanges: Arc::new(FailingExchanges),
        clock: Arc::new(SystemClock),
        config: netra_core::config::NetraConfig::with_default_data_dir(),
    };
    let runtime =
        CaptureRuntime::start(context, session, vec![Box::new(DemoCaptureProvider::new())])
            .await
            .unwrap();
    let mut events = runtime.subscribe();
    let warning = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(AppEvent::Warning { message }) => return Some(message),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    })
    .await
    .unwrap();
    let message = warning.expect("a storage failure must produce a warning");
    assert!(message.contains("storage write failed"));
    runtime.stop().await.unwrap();
}

/// Provider whose stream is empty, to prove an idle runtime shuts down.
#[derive(Debug)]
struct EmptyProvider {
    started: bool,
}

#[async_trait]
impl CaptureProvider for EmptyProvider {
    fn source(&self) -> netra_core::domain::CaptureSource {
        netra_core::domain::CaptureSource::Cdp
    }

    async fn start(&mut self) -> Result<(), netra_core::error::CaptureError> {
        self.started = true;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), netra_core::error::CaptureError> {
        Ok(())
    }

    async fn next_event(
        &mut self,
    ) -> Result<Option<netra_core::domain::CaptureEvent>, netra_core::error::CaptureError> {
        Ok(None)
    }
}

#[tokio::test]
async fn runtime_without_providers_is_rejected_explicitly() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let context = context(store);
    let session = CaptureSession::new(
        "empty",
        SourceSelection::Cdp,
        netra_core::domain::Timestamp::now(),
    )
    .unwrap();
    assert!(
        CaptureRuntime::start(context, session, Vec::new())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn provider_that_ends_closes_the_runtime() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let context = context(store.clone());
    let sessions = SessionService::new(context.clone());
    let session = sessions.create("idle", SourceSelection::Cdp).await.unwrap();
    let runtime = CaptureRuntime::start(
        context,
        session.clone(),
        vec![Box::new(EmptyProvider { started: false })],
    )
    .await
    .unwrap();
    let mut events = runtime.subscribe();
    let stopped = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(AppEvent::Stopped { reason, .. }) => return Some(reason),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    })
    .await
    .unwrap();
    assert!(stopped.is_some());
    assert_eq!(
        sessions.get(session.id).await.unwrap().status,
        SessionStatus::Stopped
    );
}

#[tokio::test]
async fn clock_abstraction_matches_system_clock() {
    let clock = SystemClock;
    assert!(clock.now().unix_millis() > 0);
}
