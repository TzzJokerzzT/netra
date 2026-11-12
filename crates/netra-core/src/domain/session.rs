//! Capture sessions.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    exchange::SourceSelection,
    ids::SessionId,
    time::{DurationMs, Timestamp},
};
use crate::error::DomainError;

/// Longest accepted session name.
pub const MAX_SESSION_NAME_LEN: usize = 120;

/// Lifecycle state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Capture is running.
    Capturing,
    /// Capture is running but events are discarded.
    Paused,
    /// Capture finished; the session is read-only.
    Stopped,
}

impl SessionStatus {
    /// Stable database and CLI spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Capturing => "capturing",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }

    /// Parses the stable spelling.
    ///
    /// # Errors
    ///
    /// Returns the offending text when it is not a known status.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "capturing" => Ok(Self::Capturing),
            "paused" => Ok(Self::Paused),
            "stopped" => Ok(Self::Stopped),
            other => Err(other.to_owned()),
        }
    }

    /// `true` while the session still accepts events.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Capturing | Self::Paused)
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A capture session aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureSession {
    /// Session identifier.
    pub id: SessionId,
    /// Human-readable name.
    pub name: String,
    /// Lifecycle status.
    pub status: SessionStatus,
    /// Which sources this session captures from.
    pub source: SourceSelection,
    /// When the session was created.
    pub created_at: Timestamp,
    /// When capture stopped, if it did.
    pub finished_at: Option<Timestamp>,
    /// When the user last saved the session explicitly.
    pub saved_at: Option<Timestamp>,
}

impl CaptureSession {
    /// Creates a session in `Capturing` state with an explicit timestamp.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSessionName`] when the trimmed name is
    /// empty or longer than [`MAX_SESSION_NAME_LEN`].
    pub fn new(
        name: impl Into<String>,
        source: SourceSelection,
        created_at: Timestamp,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            id: SessionId::new(),
            name: validate_name(name.into())?,
            status: SessionStatus::Capturing,
            source,
            created_at,
            finished_at: None,
            saved_at: None,
        })
    }

    /// Renames the session.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSessionName`] for unusable names.
    pub fn rename(&mut self, name: impl Into<String>) -> Result<(), DomainError> {
        self.name = validate_name(name.into())?;
        Ok(())
    }

    /// Marks capture as paused. Stopped sessions are not resurrected.
    pub fn pause(&mut self) {
        if self.status == SessionStatus::Capturing {
            self.status = SessionStatus::Paused;
        }
    }

    /// Marks capture as running again.
    pub fn resume(&mut self) {
        if self.status == SessionStatus::Paused {
            self.status = SessionStatus::Capturing;
        }
    }

    /// Stops capture and records the end instant.
    pub fn stop(&mut self, at: Timestamp) {
        if self.status != SessionStatus::Stopped {
            self.status = SessionStatus::Stopped;
            self.finished_at = Some(at);
        }
    }

    /// Records an explicit user save.
    pub fn mark_saved(&mut self, at: Timestamp) {
        self.saved_at = Some(at);
    }

    /// `true` when the session never stopped gracefully — it must be recovered
    /// on startup instead of being shown as live.
    #[must_use]
    pub fn is_interrupted(&self) -> bool {
        self.status.is_active()
    }

    /// Wall-clock lifetime: end when finished, otherwise up to `now`.
    #[must_use]
    pub fn duration(&self, now: Timestamp) -> DurationMs {
        let end = self.finished_at.unwrap_or(now);
        self.created_at.elapsed_until(end)
    }
}

/// Session row enriched with derived counters for list views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identifier.
    pub id: SessionId,
    /// Human-readable name.
    pub name: String,
    /// Lifecycle status.
    pub status: SessionStatus,
    /// Configured sources.
    pub source: SourceSelection,
    /// When the session was created.
    pub created_at: Timestamp,
    /// When capture stopped.
    pub finished_at: Option<Timestamp>,
    /// When the session was explicitly saved.
    pub saved_at: Option<Timestamp>,
    /// Number of persisted exchanges.
    pub exchange_count: u64,
    /// Number of failed or aborted exchanges.
    pub error_count: u64,
}

fn validate_name(name: String) -> Result<String, DomainError> {
    let trimmed = name.trim().to_owned();
    if trimmed.is_empty() {
        return Err(DomainError::InvalidSessionName {
            reason: "name must not be empty".to_owned(),
        });
    }
    if trimmed.chars().count() > MAX_SESSION_NAME_LEN {
        return Err(DomainError::InvalidSessionName {
            reason: format!("name must be at most {MAX_SESSION_NAME_LEN} characters"),
        });
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> CaptureSession {
        CaptureSession::new(
            "checkout flow",
            SourceSelection::Cdp,
            Timestamp::from_unix_millis(0),
        )
        .unwrap()
    }

    #[test]
    fn new_sessions_are_capturing_and_unsaved() {
        let created = session();
        assert_eq!(created.status, SessionStatus::Capturing);
        assert!(created.saved_at.is_none());
        assert!(created.is_interrupted());
    }

    #[test]
    fn names_are_trimmed_and_validated() {
        let created = CaptureSession::new(
            "   api debug  ",
            SourceSelection::Both,
            Timestamp::from_unix_millis(0),
        )
        .unwrap();
        assert_eq!(created.name, "api debug");
        assert!(CaptureSession::new("   ", SourceSelection::Cdp, Timestamp::now()).is_err());
        assert!(
            CaptureSession::new(
                "x".repeat(MAX_SESSION_NAME_LEN + 1),
                SourceSelection::Cdp,
                Timestamp::now()
            )
            .is_err()
        );
    }

    #[test]
    fn pause_and_resume_are_symmetric() {
        let mut created = session();
        created.pause();
        assert_eq!(created.status, SessionStatus::Paused);
        created.resume();
        assert_eq!(created.status, SessionStatus::Capturing);
    }

    #[test]
    fn stopping_is_terminal_and_records_finish_time() {
        let mut created = session();
        created.stop(Timestamp::from_unix_millis(4_000));
        assert_eq!(created.status, SessionStatus::Stopped);
        assert!(!created.is_interrupted());
        created.resume();
        assert_eq!(created.status, SessionStatus::Stopped);
        assert_eq!(
            created.duration(Timestamp::from_unix_millis(9_000)),
            DurationMs::from_millis(4_000)
        );
    }

    #[test]
    fn live_sessions_measure_against_now() {
        let created = session();
        assert_eq!(
            created.duration(Timestamp::from_unix_millis(1_500)),
            DurationMs::from_millis(1_500)
        );
    }

    #[test]
    fn saving_records_the_explicit_save_instant() {
        let mut created = session();
        created.mark_saved(Timestamp::from_unix_millis(10));
        assert_eq!(created.saved_at, Some(Timestamp::from_unix_millis(10)));
    }
}
