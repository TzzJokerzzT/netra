//! Session use cases.

use netra_core::{
    domain::{
        CaptureSession, SessionId, SessionStatus, SessionSummary, SourceSelection, Timestamp,
    },
    error::StorageError,
    ports::{ExchangeCounts, ProcessProbe},
};

use crate::{ApplicationError, ApplicationResult, context::AppContext};

/// Lifecycle operations on capture sessions.
#[derive(Debug, Clone)]
pub struct SessionService {
    context: AppContext,
}

impl SessionService {
    /// Builds the service.
    #[must_use]
    pub fn new(context: AppContext) -> Self {
        Self { context }
    }

    /// Creates a new session in `capturing` state.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Domain`] for an unusable name and
    /// [`ApplicationError::Storage`] when the row cannot be written.
    pub async fn create(
        &self,
        name: &str,
        source: SourceSelection,
    ) -> ApplicationResult<CaptureSession> {
        let session = CaptureSession::new(name, source, self.context.clock.now())?;
        self.context.sessions.create(&session).await?;
        Ok(session)
    }

    /// Loads one session.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::NotFound`] when the session does not exist.
    pub async fn get(&self, id: SessionId) -> ApplicationResult<CaptureSession> {
        self.context
            .sessions
            .get(id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound("session", id.to_string()))
    }

    /// Lists sessions, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on query failure.
    pub async fn list(&self) -> ApplicationResult<Vec<SessionSummary>> {
        Ok(self.context.sessions.list().await?)
    }

    /// Renames a session.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Domain`] for an unusable name.
    pub async fn rename(&self, id: SessionId, name: &str) -> ApplicationResult<CaptureSession> {
        let mut session = self.get(id).await?;
        session.rename(name)?;
        self.context.sessions.update(&session).await?;
        Ok(session)
    }

    /// Marks the session as capturing.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Conflict`] when the session already stopped.
    pub async fn start_capture(&self, id: SessionId) -> ApplicationResult<CaptureSession> {
        let mut session = self.get(id).await?;
        if session.status == SessionStatus::Stopped {
            return Err(ApplicationError::Conflict(format!(
                "session {} already stopped",
                session.id
            )));
        }
        session.resume();
        self.context.sessions.update(&session).await?;
        Ok(session)
    }

    /// Pauses recording.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Conflict`] when the session already stopped.
    pub async fn pause(&self, id: SessionId) -> ApplicationResult<CaptureSession> {
        let mut session = self.get(id).await?;
        if session.status == SessionStatus::Stopped {
            return Err(ApplicationError::Conflict(format!(
                "session {} already stopped",
                session.id
            )));
        }
        session.pause();
        self.context.sessions.update(&session).await?;
        Ok(session)
    }

    /// Resumes recording.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Conflict`] when the session already stopped.
    pub async fn resume(&self, id: SessionId) -> ApplicationResult<CaptureSession> {
        let mut session = self.get(id).await?;
        if session.status == SessionStatus::Stopped {
            return Err(ApplicationError::Conflict(format!(
                "session {} already stopped",
                session.id
            )));
        }
        session.resume();
        self.context.sessions.update(&session).await?;
        Ok(session)
    }

    /// Stops recording and records the end instant.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on write failure.
    pub async fn stop(&self, id: SessionId) -> ApplicationResult<CaptureSession> {
        let mut session = self.get(id).await?;
        session.stop(self.context.clock.now());
        self.context.sessions.update(&session).await?;
        Ok(session)
    }

    /// Marks the session as explicitly saved. Capture data is already persisted
    /// incrementally; this only records the user's save instant.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on write failure.
    pub async fn save(&self, id: SessionId) -> ApplicationResult<CaptureSession> {
        let mut session = self.get(id).await?;
        session.mark_saved(self.context.clock.now());
        self.context.sessions.update(&session).await?;
        Ok(session)
    }

    /// Deletes a session and its exchanges.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::NotFound`] when the session does not exist.
    pub async fn delete(&self, id: SessionId) -> ApplicationResult<()> {
        match self.context.sessions.delete(id).await {
            Ok(()) => Ok(()),
            Err(StorageError::NotFound(_)) => {
                Err(ApplicationError::NotFound("session", id.to_string()))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Exchange counters for a session.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on query failure.
    pub async fn counts(&self, id: SessionId) -> ApplicationResult<ExchangeCounts> {
        Ok(self.context.exchanges.counts(id).await?)
    }

    /// Marks every session abandoned by a dead process as stopped.
    ///
    /// A session whose recorded owner process is still running is left alone:
    /// another `netra` may be capturing into it right now.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on write failure.
    pub async fn recover_interrupted(
        &self,
        probe: &dyn ProcessProbe,
    ) -> ApplicationResult<Vec<SessionId>> {
        Ok(self
            .context
            .sessions
            .recover_interrupted(self.context.clock.now(), probe)
            .await?)
    }

    /// Elapsed lifetime of a session, using the supplied instant as "now".
    #[must_use]
    pub fn elapsed(&self, session: &CaptureSession) -> netra_core::domain::DurationMs {
        session.duration(self.context.clock.now())
    }

    /// Current instant according to the configured clock.
    #[must_use]
    pub fn now(&self) -> Timestamp {
        self.context.clock.now()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use netra_core::ports::SystemClock;
    use netra_storage::SqliteStore;

    use super::*;

    fn service() -> SessionService {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        SessionService::new(AppContext {
            sessions: store.clone(),
            exchanges: store,
            clock: Arc::new(SystemClock),
            config: netra_core::config::NetraConfig::with_default_data_dir(),
        })
    }

    #[tokio::test]
    async fn create_then_list_returns_the_session() {
        let service = service();
        let created = service
            .create("checkout", SourceSelection::Both)
            .await
            .unwrap();
        let listed = service.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
        assert_eq!(listed[0].status, SessionStatus::Capturing);
        assert_eq!(listed[0].exchange_count, 0);
    }

    #[tokio::test]
    async fn empty_name_is_rejected_before_writing() {
        let service = service();
        assert!(service.create("   ", SourceSelection::Cdp).await.is_err());
        assert!(service.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn pause_and_resume_are_persisted() {
        let service = service();
        let created = service.create("api", SourceSelection::Proxy).await.unwrap();
        assert_eq!(
            service.pause(created.id).await.unwrap().status,
            SessionStatus::Paused
        );
        assert_eq!(
            service.resume(created.id).await.unwrap().status,
            SessionStatus::Capturing
        );
        assert_eq!(
            service.get(created.id).await.unwrap().status,
            SessionStatus::Capturing
        );
    }

    #[tokio::test]
    async fn stopping_is_final_and_saving_records_the_instant() {
        let service = service();
        let created = service.create("api", SourceSelection::Cdp).await.unwrap();
        let stopped = service.stop(created.id).await.unwrap();
        assert_eq!(stopped.status, SessionStatus::Stopped);
        assert!(stopped.finished_at.is_some());
        assert!(service.pause(created.id).await.is_err());
        let saved = service.save(created.id).await.unwrap();
        assert!(saved.saved_at.is_some());
    }

    #[tokio::test]
    async fn rename_validates_and_persists() {
        let service = service();
        let created = service.create("api", SourceSelection::Cdp).await.unwrap();
        assert_eq!(
            service.rename(created.id, "  api v2 ").await.unwrap().name,
            "api v2"
        );
        assert!(service.rename(created.id, "").await.is_err());
    }

    #[tokio::test]
    async fn delete_removes_the_session_once() {
        let service = service();
        let created = service.create("api", SourceSelection::Cdp).await.unwrap();
        service.delete(created.id).await.unwrap();
        assert!(matches!(
            service.delete(created.id).await,
            Err(ApplicationError::NotFound(..))
        ));
        assert!(service.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn interrupted_sessions_are_recovered_on_startup() {
        let service = service();
        let first = service.create("one", SourceSelection::Cdp).await.unwrap();
        let second = service.create("two", SourceSelection::Proxy).await.unwrap();
        let stopped = service.stop(second.id).await.unwrap();

        let recovered = service
            .recover_interrupted(&netra_core::ports::FixedProcessProbe::default())
            .await
            .unwrap();
        assert_eq!(recovered, vec![first.id]);
        assert_eq!(
            service.get(first.id).await.unwrap().status,
            SessionStatus::Stopped
        );
        assert_eq!(
            service.get(stopped.id).await.unwrap().status,
            SessionStatus::Stopped
        );
        assert!(
            service
                .recover_interrupted(&netra_core::ports::FixedProcessProbe::default())
                .await
                .unwrap()
                .is_empty()
        );
    }
}
