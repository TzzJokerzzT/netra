//! `netra-storage` — SQLite persistence for sessions and exchanges.
//!
//! Design notes:
//!
//! * One connection behind a mutex, and every query runs inside
//!   [`tokio::task::spawn_blocking`]. SQLite is synchronous; it never runs on a
//!   runtime worker thread and never blocks the UI.
//! * WAL journaling, foreign keys and a busy timeout are enabled at open time.
//! * Capture is written incrementally: [`ExchangeRepository::apply`] turns one
//!   processor update into one transaction.
//! * Database files are created with private permissions.

#![forbid(unsafe_code)]

mod exchanges;
mod mapper;
pub mod migrations;
mod sessions;

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use netra_core::error::{StorageError, StorageResult};
use rusqlite::Connection;

pub use sessions::{SessionRow, exchange_counts};

/// SQLite-backed implementation of the persistence ports.
#[derive(Clone)]
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqliteStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// Opens (or creates) the database at `path`, creating its directory.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] when the file cannot be opened or the
    /// pragmas and migrations cannot be applied.
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                StorageError::Backend(format!("cannot create {}: {error}", parent.display()))
            })?;
            restrict_directory(parent);
        }
        let mut connection = Connection::open(&path).map_err(|error| {
            StorageError::Backend(format!("cannot open {}: {error}", path.display()))
        })?;
        configure(&connection)?;
        migrations::apply(&mut connection)
            .map_err(|error| StorageError::Backend(format!("migrations failed: {error}")))?;
        drop(connection);
        restrict_file(&path);
        let connection = Connection::open(&path).map_err(|error| {
            StorageError::Backend(format!("cannot reopen {}: {error}", path.display()))
        })?;
        configure(&connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path,
        })
    }

    /// Opens a private in-memory database, used by tests.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] when migrations fail.
    pub fn open_in_memory() -> StorageResult<Self> {
        let mut connection = Connection::open_in_memory()
            .map_err(|error| StorageError::Backend(format!("cannot open in-memory db: {error}")))?;
        configure(&connection)?;
        migrations::apply(&mut connection)
            .map_err(|error| StorageError::Backend(format!("migrations failed: {error}")))?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path: PathBuf::from(":memory:"),
        })
    }

    /// Database file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Schema version currently applied.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] when the pragma cannot be read.
    pub fn schema_version(&self) -> StorageResult<usize> {
        let connection = self.lock()?;
        migrations::current_version(&connection)
            .map_err(|error| StorageError::Backend(error.to_string()))
    }

    /// Runs `operation` on the blocking pool with exclusive access.
    pub(crate) async fn with_connection<F, T>(&self, operation: F) -> StorageResult<T>
    where
        F: FnOnce(&mut Connection) -> StorageResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let mut guard = connection
                .lock()
                .map_err(|_| StorageError::Backend("connection mutex poisoned".to_owned()))?;
            operation(&mut guard)
        })
        .await
        .map_err(|error| StorageError::Backend(format!("sqlite worker failed: {error}")))?
    }

    fn lock(&self) -> StorageResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| StorageError::Backend("connection mutex poisoned".to_owned()))
    }
}

/// Applies the pragmas every `netra` connection needs.
fn configure(connection: &Connection) -> StorageResult<()> {
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;",
        )
        .map_err(|error| StorageError::Backend(format!("cannot configure sqlite: {error}")))?;
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        let _ = std::fs::set_permissions(path, permissions);
    }
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) {}

#[cfg(unix)]
fn restrict_directory(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o700);
        let _ = std::fs::set_permissions(path, permissions);
    }
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) {}

/// Maps a rusqlite error to the storage error the ports expose.
fn map_error(error: rusqlite::Error, context: &str) -> StorageError {
    use rusqlite::ErrorCode;
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == ErrorCode::ConstraintViolation =>
        {
            StorageError::Integrity(format!("{context}: {error}"))
        }
        _ => StorageError::Backend(format!("{context}: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_applies_migrations_and_wal() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("netra.db")).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            migrations::latest_version()
        );
        let connection = store.lock().unwrap();
        let mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[cfg(unix)]
    #[test]
    fn database_files_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("netra.db");
        let _store = SqliteStore::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn reopening_keeps_the_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("netra.db");
        let first = SqliteStore::open(&path).unwrap().schema_version().unwrap();
        let second = SqliteStore::open(&path).unwrap().schema_version().unwrap();
        assert_eq!(first, second);
    }
}
