//! Session persistence.

use async_trait::async_trait;
use netra_core::{
    domain::{CaptureSession, SessionId, SessionSummary, Timestamp},
    error::{StorageError, StorageResult},
    ports::{ExchangeCounts, SessionRepository},
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    SqliteStore,
    mapper::{
        millis, opt_millis, opt_timestamp, session_id, session_status, source_selection, timestamp,
    },
};

/// One `sessions` row with derived counters.
#[derive(Debug, Clone)]
pub struct SessionRow {
    /// Session identifier as stored.
    pub id: String,
    /// Session name.
    pub name: String,
    /// Stored status.
    pub status: String,
    /// Stored source selection.
    pub source: String,
    /// Creation instant in Unix milliseconds.
    pub created_at: i64,
    /// Stop instant in Unix milliseconds.
    pub finished_at: Option<i64>,
    /// Explicit save instant in Unix milliseconds.
    pub saved_at: Option<i64>,
    /// Persisted exchange count.
    pub exchange_count: i64,
    /// Persisted failed/aborted exchange count.
    pub error_count: i64,
}

impl SessionRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            status: row.get("status")?,
            source: row.get("source")?,
            created_at: row.get("created_at")?,
            finished_at: row.get("finished_at")?,
            saved_at: row.get("saved_at")?,
            exchange_count: row.get("exchange_count")?,
            error_count: row.get("error_count")?,
        })
    }

    /// Projects the row into the list summary.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Decode`] for unknown enums or identifiers.
    pub fn to_summary(&self) -> StorageResult<SessionSummary> {
        Ok(SessionSummary {
            id: session_id(&self.id)?,
            name: self.name.clone(),
            status: session_status(&self.status)?,
            source: source_selection(&self.source)?,
            created_at: timestamp(self.created_at),
            finished_at: opt_timestamp(self.finished_at),
            saved_at: opt_timestamp(self.saved_at),
            exchange_count: u64::try_from(self.exchange_count).unwrap_or(0),
            error_count: u64::try_from(self.error_count).unwrap_or(0),
        })
    }

    /// Rebuilds the aggregate from the row.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Decode`] for unknown enums or identifiers.
    pub fn to_session(&self) -> StorageResult<CaptureSession> {
        let summary = self.to_summary()?;
        Ok(CaptureSession {
            id: summary.id,
            name: summary.name,
            status: summary.status,
            source: summary.source,
            created_at: summary.created_at,
            finished_at: summary.finished_at,
            saved_at: summary.saved_at,
        })
    }
}

const SESSION_SELECT: &str = "SELECT s.id, s.name, s.status, s.source, s.created_at, s.finished_at, \
     s.saved_at, \
     (SELECT COUNT(*) FROM exchanges e WHERE e.session_id = s.id) AS exchange_count, \
     (SELECT COUNT(*) FROM exchanges e WHERE e.session_id = s.id AND e.state IN ('failed', 'aborted')) \
        AS error_count \
     FROM sessions s";

/// Counts exchanges of a session by state.
///
/// # Errors
///
/// Returns [`StorageError::Backend`] when the aggregate query fails.
pub fn exchange_counts(
    connection: &Connection,
    session_id: SessionId,
) -> StorageResult<ExchangeCounts> {
    let mut statement = connection
        .prepare("SELECT state, COUNT(*) FROM exchanges WHERE session_id = ?1 GROUP BY state")
        .map_err(|error| StorageError::Backend(error.to_string()))?;
    let rows = statement
        .query_map(params![session_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| StorageError::Backend(error.to_string()))?;
    let mut counts = ExchangeCounts::default();
    for row in rows {
        let (state, count) = row.map_err(|error| StorageError::Backend(error.to_string()))?;
        let count = u64::try_from(count).unwrap_or(0);
        counts.total += count;
        match state.as_str() {
            "failed" | "aborted" => counts.errors += count,
            "pending" => counts.pending += count,
            _ => {}
        }
    }
    Ok(counts)
}

#[async_trait]
impl SessionRepository for SqliteStore {
    async fn create(&self, session: &CaptureSession) -> StorageResult<()> {
        let session = session.clone();
        self.with_connection(move |connection| {
            connection
                .execute(
                    "INSERT INTO sessions (id, name, status, source, created_at, finished_at, saved_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        session.id.to_string(),
                        session.name,
                        session.status.as_str(),
                        session.source.as_str(),
                        millis(session.created_at),
                        opt_millis(session.finished_at),
                        opt_millis(session.saved_at),
                    ],
                )
                .map_err(|error| crate::map_error(error, "insert session"))?;
            Ok(())
        })
        .await
    }

    async fn update(&self, session: &CaptureSession) -> StorageResult<()> {
        let session = session.clone();
        self.with_connection(move |connection| {
            let changed = connection
                .execute(
                    "UPDATE sessions SET name = ?2, status = ?3, source = ?4, finished_at = ?5,
                            saved_at = ?6
                     WHERE id = ?1",
                    params![
                        session.id.to_string(),
                        session.name,
                        session.status.as_str(),
                        session.source.as_str(),
                        opt_millis(session.finished_at),
                        opt_millis(session.saved_at),
                    ],
                )
                .map_err(|error| crate::map_error(error, "update session"))?;
            if changed == 0 {
                return Err(StorageError::NotFound(format!("session {}", session.id)));
            }
            Ok(())
        })
        .await
    }

    async fn get(&self, id: SessionId) -> StorageResult<Option<CaptureSession>> {
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(&format!("{SESSION_SELECT} WHERE s.id = ?1"))
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            let row = statement
                .query_row(params![id.to_string()], SessionRow::read)
                .optional()
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            row.map(|row| row.to_session()).transpose()
        })
        .await
    }

    async fn list(&self) -> StorageResult<Vec<SessionSummary>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(&format!(
                    "{SESSION_SELECT} ORDER BY s.created_at DESC, s.id"
                ))
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            let rows = statement
                .query_map([], SessionRow::read)
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            let mut sessions = Vec::new();
            for row in rows {
                let row = row.map_err(|error| StorageError::Backend(error.to_string()))?;
                sessions.push(row.to_summary()?);
            }
            Ok(sessions)
        })
        .await
    }

    async fn delete(&self, id: SessionId) -> StorageResult<()> {
        self.with_connection(move |connection| {
            let changed = connection
                .execute(
                    "DELETE FROM sessions WHERE id = ?1",
                    params![id.to_string()],
                )
                .map_err(|error| crate::map_error(error, "delete session"))?;
            if changed == 0 {
                return Err(StorageError::NotFound(format!("session {id}")));
            }
            Ok(())
        })
        .await
    }

    async fn recover_interrupted(&self, at: Timestamp) -> StorageResult<Vec<SessionId>> {
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare("SELECT id FROM sessions WHERE status IN ('capturing', 'paused') ORDER BY created_at")
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            let ids: Vec<String> = statement
                .query_map([], |row| row.get(0))
                .map_err(|error| StorageError::Backend(error.to_string()))?
                .map(|row| row.map_err(|error| StorageError::Backend(error.to_string())))
                .collect::<StorageResult<Vec<String>>>()?;
            drop(statement);
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let transaction = connection
                .transaction()
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            transaction
                .execute(
                    "UPDATE sessions SET status = 'stopped', finished_at = COALESCE(finished_at, ?1)
                     WHERE status IN ('capturing', 'paused')",
                    params![millis(at)],
                )
                .map_err(|error| crate::map_error(error, "recover sessions"))?;
            transaction
                .commit()
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            ids.iter()
                .map(|id| {
                    SessionId::parse(id).map_err(|error| StorageError::Decode(error.to_string()))
                })
                .collect()
        })
        .await
    }
}
