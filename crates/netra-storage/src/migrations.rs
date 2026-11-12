//! Versioned schema migrations.
//!
//! `PRAGMA user_version` records the applied version. Each migration runs inside
//! a transaction together with its version bump, so an interrupted migration
//! leaves the database at the previous version instead of half-upgraded.

use rusqlite::{Connection, Transaction};

/// Ordered migrations. Never edit or reorder an applied entry: add a new one.
pub const MIGRATIONS: &[&str] = &[M1_INITIAL_SCHEMA];

/// Version 1: sessions, exchanges, headers, bodies and timings.
const M1_INITIAL_SCHEMA: &str = r"
CREATE TABLE sessions (
    id           TEXT PRIMARY KEY NOT NULL,
    name         TEXT NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('capturing', 'paused', 'stopped')),
    source       TEXT NOT NULL CHECK (source IN ('cdp', 'proxy', 'both')),
    created_at   INTEGER NOT NULL,
    finished_at  INTEGER,
    saved_at     INTEGER
);

CREATE INDEX idx_sessions_created_at ON sessions (created_at DESC);
CREATE INDEX idx_sessions_status ON sessions (status);

CREATE TABLE exchanges (
    id                   TEXT PRIMARY KEY NOT NULL,
    session_id           TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    source_request_id    TEXT,
    method               TEXT NOT NULL,
    url                  TEXT NOT NULL,
    host                 TEXT,
    status_code          INTEGER,
    content_type         TEXT,
    media_kind           TEXT NOT NULL,
    duration_ms          INTEGER,
    request_size_bytes   INTEGER NOT NULL DEFAULT 0,
    response_size_bytes  INTEGER NOT NULL DEFAULT 0,
    started_at           INTEGER NOT NULL,
    finished_at          INTEGER,
    state                TEXT NOT NULL CHECK (state IN ('pending', 'completed', 'failed', 'aborted')),
    source               TEXT NOT NULL CHECK (source IN ('cdp', 'proxy')),
    error                TEXT,
    http_version         TEXT,
    request_http_version TEXT
);

CREATE INDEX idx_exchanges_session_started ON exchanges (session_id, started_at DESC);
CREATE INDEX idx_exchanges_session_state ON exchanges (session_id, state);
CREATE INDEX idx_exchanges_session_method ON exchanges (session_id, method);
CREATE INDEX idx_exchanges_session_status ON exchanges (session_id, status_code);
CREATE INDEX idx_exchanges_host ON exchanges (host);
CREATE INDEX idx_exchanges_source_request ON exchanges (source, source_request_id);

CREATE TABLE headers (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    exchange_id TEXT NOT NULL REFERENCES exchanges (id) ON DELETE CASCADE,
    direction   TEXT NOT NULL CHECK (direction IN ('request', 'response')),
    position    INTEGER NOT NULL,
    name        TEXT NOT NULL,
    value       TEXT NOT NULL
);

CREATE INDEX idx_headers_exchange ON headers (exchange_id, direction, position);

CREATE TABLE bodies (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    exchange_id  TEXT NOT NULL REFERENCES exchanges (id) ON DELETE CASCADE,
    direction    TEXT NOT NULL CHECK (direction IN ('request', 'response')),
    state        TEXT NOT NULL CHECK (state IN ('absent', 'captured', 'omitted', 'redacted')),
    omit_reason  TEXT,
    content_type TEXT,
    encoding     TEXT,
    data         BLOB,
    size_bytes   INTEGER NOT NULL DEFAULT 0,
    truncated    INTEGER NOT NULL DEFAULT 0,
    UNIQUE (exchange_id, direction)
);

CREATE TABLE timings (
    exchange_id TEXT PRIMARY KEY NOT NULL REFERENCES exchanges (id) ON DELETE CASCADE,
    blocked_ms  INTEGER,
    dns_ms      INTEGER,
    connect_ms  INTEGER,
    tls_ms      INTEGER,
    ttfb_ms     INTEGER,
    download_ms INTEGER,
    total_ms    INTEGER
);
";

/// Applies every migration the database is missing.
///
/// # Errors
///
/// Returns the underlying SQLite error, leaving the database at its previous
/// version.
pub fn apply(connection: &mut Connection) -> rusqlite::Result<()> {
    let current: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let applied = usize::try_from(current.max(0)).unwrap_or(0);
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
        let version = index + 1;
        let transaction: Transaction<'_> = connection.transaction()?;
        transaction.execute_batch(migration)?;
        transaction.pragma_update(None, "user_version", version as i64)?;
        transaction.commit()?;
    }
    Ok(())
}

/// Schema version the database should be at after [`apply`].
#[must_use]
pub fn latest_version() -> usize {
    MIGRATIONS.len()
}

/// Reads the schema version recorded in the database.
///
/// # Errors
///
/// Returns the underlying SQLite error.
pub fn current_version(connection: &Connection) -> rusqlite::Result<usize> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(usize::try_from(version).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_once_and_are_idempotent() {
        let mut connection = Connection::open_in_memory().unwrap();
        apply(&mut connection).unwrap();
        assert_eq!(current_version(&connection).unwrap(), latest_version());
        apply(&mut connection).unwrap();
        assert_eq!(current_version(&connection).unwrap(), latest_version());
    }

    #[test]
    fn schema_has_the_expected_tables() {
        let mut connection = Connection::open_in_memory().unwrap();
        apply(&mut connection).unwrap();
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert!(tables.contains(&"sessions".to_owned()));
        assert!(tables.contains(&"exchanges".to_owned()));
        assert!(tables.contains(&"headers".to_owned()));
        assert!(tables.contains(&"bodies".to_owned()));
        assert!(tables.contains(&"timings".to_owned()));
    }

    #[test]
    fn state_checks_reject_unknown_values() {
        let mut connection = Connection::open_in_memory().unwrap();
        apply(&mut connection).unwrap();
        connection
            .execute(
                "INSERT INTO sessions (id, name, status, source, created_at) VALUES ('s', 'n', 'capturing', 'cdp', 0)",
                [],
            )
            .unwrap();
        let result = connection.execute(
            "INSERT INTO exchanges (id, session_id, method, url, media_kind, started_at, state, source)
             VALUES ('e', 's', 'GET', 'https://a.test/', 'json', 0, 'not-a-state', 'cdp')",
            [],
        );
        assert!(result.is_err());
    }
}
