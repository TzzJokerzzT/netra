//! Exchange persistence and querying.

use async_trait::async_trait;
use netra_core::{
    domain::{
        Body, BodyDirection, ContentType, ExchangeId, ExchangeQuery, ExchangeState,
        ExchangeSummary, ExchangeUpdate, HttpHeaders, MediaKind, NetworkExchange, NetworkResponse,
        SessionId, TimingMetrics,
    },
    error::{StorageError, StorageResult},
    ports::{ExchangeCounts, ExchangeRepository},
};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};

use crate::{
    SqliteStore,
    mapper::{BodyRow, EXCHANGE_COLUMNS, ExchangeRow, exchange_id, millis, timing_values},
    sessions::exchange_counts,
};

#[async_trait]
impl ExchangeRepository for SqliteStore {
    async fn upsert(&self, exchange: &NetworkExchange) -> StorageResult<()> {
        let exchange = exchange.clone();
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            insert_exchange(&transaction, &exchange)?;
            transaction
                .commit()
                .map_err(|error| StorageError::Backend(error.to_string()))
        })
        .await
    }

    async fn apply(&self, update: &ExchangeUpdate) -> StorageResult<()> {
        let update = update.clone();
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            match &update {
                ExchangeUpdate::Discovered(exchange) => insert_exchange(&transaction, exchange)?,
                ExchangeUpdate::Redirected {
                    previous_id,
                    previous,
                    next,
                } => {
                    apply_response(&transaction, *previous_id, previous)?;
                    insert_exchange(&transaction, next)?;
                }
                ExchangeUpdate::Response { id, response } => {
                    apply_response(&transaction, *id, response)?;
                }
                ExchangeUpdate::Body {
                    id,
                    direction,
                    body,
                } => {
                    replace_body(&transaction, *id, *direction, body)?;
                    update_sizes(&transaction, *id)?;
                    touch_content_type(&transaction, *id)?;
                }
                ExchangeUpdate::Timing { id, timing } => {
                    upsert_timing(&transaction, *id, timing)?;
                    let total = timing
                        .total
                        .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX));
                    transaction
                        .execute(
                            "UPDATE exchanges SET duration_ms = COALESCE(?2, duration_ms) WHERE id = ?1",
                            params![id.to_string(), total],
                        )
                        .map_err(|error| crate::map_error(error, "update timing"))?;
                }
                ExchangeUpdate::State { id, state, error } => {
                    apply_state(&transaction, *id, *state, error.as_deref())?;
                }
            }
            transaction
                .commit()
                .map_err(|error| StorageError::Backend(error.to_string()))
        })
        .await
    }

    async fn get(&self, id: ExchangeId) -> StorageResult<Option<NetworkExchange>> {
        self.with_connection(move |connection| load_exchange(connection, id))
            .await
    }

    async fn page(&self, query: &ExchangeQuery) -> StorageResult<Vec<ExchangeSummary>> {
        let query = query.clone();
        self.with_connection(move |connection| {
            let (clause, values) = filter_clause(&query)?;
            let order = match query.order {
                netra_core::domain::filter::ExchangeOrder::StartedAsc => "ASC",
                netra_core::domain::filter::ExchangeOrder::StartedDesc => "DESC",
            };
            let sql = format!(
                "SELECT {EXCHANGE_COLUMNS} FROM exchanges WHERE {clause} \
                 ORDER BY started_at {order}, id {order} LIMIT ? OFFSET ?"
            );
            let mut all: Vec<Value> = values;
            all.push(Value::Integer(
                i64::try_from(query.limit).unwrap_or(i64::MAX),
            ));
            all.push(Value::Integer(i64::try_from(query.offset).unwrap_or(0)));
            let mut statement = connection
                .prepare(&sql)
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            let rows = statement
                .query_map(params_from_iter(all), ExchangeRow::read)
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            let mut summaries = Vec::new();
            for row in rows {
                let row = row.map_err(|error| StorageError::Backend(error.to_string()))?;
                summaries.push(row.to_summary()?);
            }
            Ok(summaries)
        })
        .await
    }

    async fn counts(&self, session_id: SessionId) -> StorageResult<ExchangeCounts> {
        self.with_connection(move |connection| exchange_counts(connection, session_id))
            .await
    }

    async fn enforce_retention(&self, session_id: SessionId, keep: u64) -> StorageResult<u64> {
        self.with_connection(move |connection| {
            let keep = i64::try_from(keep).unwrap_or(i64::MAX);
            let deleted = connection
                .execute(
                    "DELETE FROM exchanges
                     WHERE session_id = ?1
                       AND id NOT IN (
                           SELECT id FROM exchanges WHERE session_id = ?1
                           ORDER BY started_at DESC, id DESC LIMIT ?2
                       )",
                    params![session_id.to_string(), keep],
                )
                .map_err(|error| crate::map_error(error, "enforce retention"))?;
            Ok(u64::try_from(deleted).unwrap_or(0))
        })
        .await
    }

    async fn hosts(&self, session_id: SessionId) -> StorageResult<Vec<(String, u64)>> {
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT host, COUNT(*) FROM exchanges
                     WHERE session_id = ?1 AND host IS NOT NULL
                     GROUP BY host ORDER BY COUNT(*) DESC, host",
                )
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            let rows = statement
                .query_map(params![session_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            let mut hosts = Vec::new();
            for row in rows {
                let (host, count) =
                    row.map_err(|error| StorageError::Backend(error.to_string()))?;
                hosts.push((host, u64::try_from(count).unwrap_or(0)));
            }
            Ok(hosts)
        })
        .await
    }

    async fn update_body(
        &self,
        id: ExchangeId,
        direction: BodyDirection,
        body: &Body,
    ) -> StorageResult<()> {
        let body = body.clone();
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            replace_body(&transaction, id, direction, &body)?;
            update_sizes(&transaction, id)?;
            touch_content_type(&transaction, id)?;
            transaction
                .commit()
                .map_err(|error| StorageError::Backend(error.to_string()))
        })
        .await
    }

    async fn update_response(
        &self,
        id: ExchangeId,
        response: &NetworkResponse,
    ) -> StorageResult<()> {
        let response = response.clone();
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            apply_response(&transaction, id, &response)?;
            transaction
                .commit()
                .map_err(|error| StorageError::Backend(error.to_string()))
        })
        .await
    }

    async fn update_state(
        &self,
        id: ExchangeId,
        state: ExchangeState,
        error: Option<&str>,
        timing: Option<TimingMetrics>,
    ) -> StorageResult<()> {
        let error = error.map(str::to_owned);
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            apply_state(&transaction, id, state, error.as_deref())?;
            if let Some(timing) = timing {
                upsert_timing(&transaction, id, &timing)?;
            }
            transaction
                .commit()
                .map_err(|error| StorageError::Backend(error.to_string()))
        })
        .await
    }
}

/// Inserts or replaces one exchange and all of its child rows.
fn insert_exchange(connection: &Connection, exchange: &NetworkExchange) -> StorageResult<()> {
    let request_size = i64::try_from(exchange.request.body.size_bytes).unwrap_or(i64::MAX);
    let response_size = exchange.response.as_ref().map_or(0, |response| {
        i64::try_from(response.body.size_bytes).unwrap_or(i64::MAX)
    });
    let finished_at = exchange
        .response
        .as_ref()
        .map(|response| millis(response.received_at));
    connection
        .execute(
            "INSERT INTO exchanges (id, session_id, source_request_id, method, url, host, status_code,
                 content_type, media_kind, duration_ms, request_size_bytes, response_size_bytes,
                 started_at, finished_at, state, source, error, http_version, request_http_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT (id) DO UPDATE SET
                 method = excluded.method,
                 url = excluded.url,
                 host = excluded.host,
                 status_code = excluded.status_code,
                 content_type = excluded.content_type,
                 media_kind = excluded.media_kind,
                 duration_ms = excluded.duration_ms,
                 request_size_bytes = excluded.request_size_bytes,
                 response_size_bytes = excluded.response_size_bytes,
                 finished_at = excluded.finished_at,
                 state = excluded.state,
                 error = excluded.error,
                 http_version = excluded.http_version,
                 request_http_version = excluded.request_http_version",
            params![
                exchange.id.to_string(),
                exchange.session_id.to_string(),
                exchange.source_request_id,
                exchange.request.method.as_str(),
                exchange.request.url.as_str(),
                exchange.host(),
                exchange.status().map(|status| status.as_u16()),
                exchange.content_type().map(ToString::to_string),
                exchange.media_kind().as_str(),
                exchange
                    .duration()
                    .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX)),
                request_size,
                response_size,
                millis(exchange.request.started_at),
                finished_at,
                exchange.state.as_str(),
                exchange.source.as_str(),
                exchange.error,
                exchange
                    .response
                    .as_ref()
                    .and_then(|response| response.http_version.clone()),
                exchange.request.http_version,
            ],
        )
        .map_err(|error| crate::map_error(error, "insert exchange"))?;
    replace_headers(
        connection,
        exchange.id,
        BodyDirection::Request,
        &exchange.request.headers,
    )?;
    replace_body(
        connection,
        exchange.id,
        BodyDirection::Request,
        &exchange.request.body,
    )?;
    if let Some(response) = exchange.response.as_ref() {
        replace_headers(
            connection,
            exchange.id,
            BodyDirection::Response,
            &response.headers,
        )?;
        replace_body(
            connection,
            exchange.id,
            BodyDirection::Response,
            &response.body,
        )?;
    }
    if let Some(timing) = exchange.timing {
        upsert_timing(connection, exchange.id, &timing)?;
    }
    Ok(())
}

/// Replaces one side's headers, preserving order and duplicates.
fn replace_headers(
    connection: &Connection,
    id: ExchangeId,
    direction: BodyDirection,
    headers: &HttpHeaders,
) -> StorageResult<()> {
    connection
        .execute(
            "DELETE FROM headers WHERE exchange_id = ?1 AND direction = ?2",
            params![id.to_string(), direction.as_str()],
        )
        .map_err(|error| crate::map_error(error, "clear headers"))?;
    let mut statement = connection
        .prepare(
            "INSERT INTO headers (exchange_id, direction, position, name, value)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .map_err(|error| StorageError::Backend(error.to_string()))?;
    for (position, entry) in headers.iter().enumerate() {
        statement
            .execute(params![
                id.to_string(),
                direction.as_str(),
                i64::try_from(position).unwrap_or(i64::MAX),
                entry.name,
                entry.value,
            ])
            .map_err(|error| crate::map_error(error, "insert header"))?;
    }
    Ok(())
}

/// Replaces one side's payload row.
fn replace_body(
    connection: &Connection,
    id: ExchangeId,
    direction: BodyDirection,
    body: &Body,
) -> StorageResult<()> {
    let row = BodyRow::from_body(body);
    connection
        .execute(
            "INSERT INTO bodies (exchange_id, direction, state, omit_reason, content_type, encoding,
                 data, size_bytes, truncated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (exchange_id, direction) DO UPDATE SET
                 state = excluded.state,
                 omit_reason = excluded.omit_reason,
                 content_type = excluded.content_type,
                 encoding = excluded.encoding,
                 data = excluded.data,
                 size_bytes = excluded.size_bytes,
                 truncated = excluded.truncated",
            params![
                id.to_string(),
                direction.as_str(),
                row.state,
                row.omit_reason,
                row.content_type,
                row.encoding,
                row.data,
                row.size_bytes,
                i64::from(row.truncated),
            ],
        )
        .map_err(|error| crate::map_error(error, "upsert body"))?;
    Ok(())
}

/// Recomputes the stored size columns from the payload rows.
fn update_sizes(connection: &Connection, id: ExchangeId) -> StorageResult<()> {
    connection
        .execute(
            "UPDATE exchanges SET
                 request_size_bytes = COALESCE((SELECT size_bytes FROM bodies
                     WHERE exchange_id = ?1 AND direction = 'request'), request_size_bytes),
                 response_size_bytes = COALESCE((SELECT size_bytes FROM bodies
                     WHERE exchange_id = ?1 AND direction = 'response'), response_size_bytes)
             WHERE id = ?1",
            params![id.to_string()],
        )
        .map_err(|error| crate::map_error(error, "update sizes"))?;
    Ok(())
}

/// Refreshes `content_type` and `media_kind` after a payload change.
fn touch_content_type(connection: &Connection, id: ExchangeId) -> StorageResult<()> {
    let content_type: Option<String> = connection
        .query_row(
            "SELECT content_type FROM bodies WHERE exchange_id = ?1 AND direction = 'response'",
            params![id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::Backend(error.to_string()))?
        .flatten();
    if let Some(content_type) = content_type {
        let kind = ContentType::parse(&content_type).kind();
        connection
            .execute(
                "UPDATE exchanges SET content_type = ?2, media_kind = ?3 WHERE id = ?1",
                params![id.to_string(), content_type, kind.as_str()],
            )
            .map_err(|error| crate::map_error(error, "update content type"))?;
    }
    Ok(())
}

/// Stores one response and marks the exchange completed.
fn apply_response(
    connection: &Connection,
    id: ExchangeId,
    response: &NetworkResponse,
) -> StorageResult<()> {
    let started_at: Option<i64> = connection
        .query_row(
            "SELECT started_at FROM exchanges WHERE id = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::Backend(error.to_string()))?;
    let Some(started_at) = started_at else {
        return Err(StorageError::NotFound(format!("exchange {id}")));
    };
    let duration = response
        .received_at
        .unix_millis()
        .saturating_sub(started_at);
    let media_kind: MediaKind = response
        .content_type()
        .map_or(MediaKind::Other, ContentType::kind);
    connection
        .execute(
            "UPDATE exchanges SET status_code = ?2, content_type = ?3, media_kind = ?4,
                    duration_ms = ?5, response_size_bytes = ?6, state = 'completed', error = NULL,
                    finished_at = ?7, http_version = ?8
             WHERE id = ?1",
            params![
                id.to_string(),
                response.status.as_u16(),
                response.content_type().map(ToString::to_string),
                media_kind.as_str(),
                duration,
                i64::try_from(response.body.size_bytes).unwrap_or(i64::MAX),
                millis(response.received_at),
                response.http_version,
            ],
        )
        .map_err(|error| crate::map_error(error, "update response"))?;
    replace_headers(connection, id, BodyDirection::Response, &response.headers)?;
    replace_body(connection, id, BodyDirection::Response, &response.body)?;
    Ok(())
}

/// Stores a terminal state and its error text.
fn apply_state(
    connection: &Connection,
    id: ExchangeId,
    state: ExchangeState,
    error: Option<&str>,
) -> StorageResult<()> {
    let finished_at = if state.is_terminal() {
        Some(millis(netra_core::domain::Timestamp::now()))
    } else {
        None
    };
    let changed = connection
        .execute(
            "UPDATE exchanges SET state = ?2, error = ?3,
                    finished_at = CASE WHEN ?4 IS NULL THEN finished_at
                                       ELSE COALESCE(finished_at, ?4) END
             WHERE id = ?1",
            params![id.to_string(), state.as_str(), error, finished_at],
        )
        .map_err(|error| crate::map_error(error, "update state"))?;
    if changed == 0 {
        return Err(StorageError::NotFound(format!("exchange {id}")));
    }
    Ok(())
}

/// Inserts or replaces the timing row.
fn upsert_timing(
    connection: &Connection,
    id: ExchangeId,
    timing: &TimingMetrics,
) -> StorageResult<()> {
    let values = timing_values(timing);
    connection
        .execute(
            "INSERT INTO timings (exchange_id, blocked_ms, dns_ms, connect_ms, tls_ms, ttfb_ms,
                 download_ms, total_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (exchange_id) DO UPDATE SET
                 blocked_ms = excluded.blocked_ms,
                 dns_ms = excluded.dns_ms,
                 connect_ms = excluded.connect_ms,
                 tls_ms = excluded.tls_ms,
                 ttfb_ms = excluded.ttfb_ms,
                 download_ms = excluded.download_ms,
                 total_ms = excluded.total_ms",
            params![
                id.to_string(),
                values[0],
                values[1],
                values[2],
                values[3],
                values[4],
                values[5],
                values[6],
            ],
        )
        .map_err(|error| crate::map_error(error, "upsert timing"))?;
    Ok(())
}

/// Loads one exchange with headers, bodies and timings.
fn load_exchange(
    connection: &Connection,
    id: ExchangeId,
) -> StorageResult<Option<NetworkExchange>> {
    let sql = format!("SELECT {EXCHANGE_COLUMNS} FROM exchanges WHERE id = ?1");
    let row = connection
        .query_row(&sql, params![id.to_string()], ExchangeRow::read)
        .optional()
        .map_err(|error| StorageError::Backend(error.to_string()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut request = row.to_request()?;
    request.headers = load_headers(connection, id, BodyDirection::Request)?;
    request.body = load_body(connection, id, BodyDirection::Request)?;
    let mut response = row.to_response();
    if let Some(response) = response.as_mut() {
        response.headers = load_headers(connection, id, BodyDirection::Response)?;
        response.body = load_body(connection, id, BodyDirection::Response)?;
    }
    let timing = load_timing(connection, id)?;
    Ok(Some(NetworkExchange {
        id: exchange_id(&row.id)?,
        session_id: netra_core::domain::SessionId::parse(&row.session_id)
            .map_err(|error| StorageError::Decode(error.to_string()))?,
        request,
        response,
        timing,
        source: crate::mapper::source(&row.source)?,
        state: crate::mapper::state(&row.state)?,
        error: row.error.clone(),
        source_request_id: row.source_request_id.clone(),
    }))
}

fn load_headers(
    connection: &Connection,
    id: ExchangeId,
    direction: BodyDirection,
) -> StorageResult<HttpHeaders> {
    let mut statement = connection
        .prepare(
            "SELECT name, value FROM headers WHERE exchange_id = ?1 AND direction = ?2
             ORDER BY position",
        )
        .map_err(|error| StorageError::Backend(error.to_string()))?;
    let rows = statement
        .query_map(params![id.to_string(), direction.as_str()], |row| {
            Ok(HttpHeaders::from_pairs([(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            )]))
        })
        .map_err(|error| StorageError::Backend(error.to_string()))?;
    let mut headers = HttpHeaders::new();
    for row in rows {
        let partial = row.map_err(|error| StorageError::Backend(error.to_string()))?;
        for entry in partial.iter() {
            headers.push(entry.name.clone(), entry.value.clone());
        }
    }
    Ok(headers)
}

fn load_body(
    connection: &Connection,
    id: ExchangeId,
    direction: BodyDirection,
) -> StorageResult<Body> {
    let row = connection
        .query_row(
            "SELECT state, omit_reason, content_type, encoding, data, size_bytes, truncated
             FROM bodies WHERE exchange_id = ?1 AND direction = ?2",
            params![id.to_string(), direction.as_str()],
            |row| {
                Ok(BodyRow {
                    state: row.get("state")?,
                    omit_reason: row.get("omit_reason")?,
                    content_type: row.get("content_type")?,
                    encoding: row.get("encoding")?,
                    data: row.get("data")?,
                    size_bytes: row.get("size_bytes")?,
                    truncated: row.get::<_, i64>("truncated")? != 0,
                })
            },
        )
        .optional()
        .map_err(|error| StorageError::Backend(error.to_string()))?;
    let Some(row) = row else {
        return Ok(Body::absent());
    };
    Ok(row.to_body())
}

fn load_timing(connection: &Connection, id: ExchangeId) -> StorageResult<Option<TimingMetrics>> {
    let row = connection
        .query_row(
            "SELECT blocked_ms, dns_ms, connect_ms, tls_ms, ttfb_ms, download_ms, total_ms
             FROM timings WHERE exchange_id = ?1",
            params![id.to_string()],
            |row| {
                Ok([
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ])
            },
        )
        .optional()
        .map_err(|error| StorageError::Backend(error.to_string()))?;
    let Some(values) = row else {
        return Ok(None);
    };
    let to_duration = |value: Option<i64>| {
        value
            .and_then(|millis| u64::try_from(millis).ok())
            .map(netra_core::domain::DurationMs::from_millis)
    };
    Ok(Some(TimingMetrics {
        blocked: to_duration(values[0]),
        dns: to_duration(values[1]),
        connect: to_duration(values[2]),
        tls: to_duration(values[3]),
        ttfb: to_duration(values[4]),
        download: to_duration(values[5]),
        total: to_duration(values[6]),
    }))
}

/// Builds the SQL predicate and bound values for a query filter.
fn filter_clause(query: &ExchangeQuery) -> StorageResult<(String, Vec<Value>)> {
    use netra_core::domain::ExchangeFilter;
    let ExchangeFilter {
        text,
        host,
        methods,
        status,
        media_kinds,
        sources,
        states,
        only_failed,
    } = &query.filter;
    let mut clauses: Vec<String> = vec!["session_id = ?".to_owned()];
    let mut values: Vec<Value> = vec![Value::Text(query.session_id.to_string())];

    if let Some(text) = text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let pattern = format!("%{}%", escape_like(text));
        clauses
            .push("(url LIKE ? ESCAPE '\\' OR COALESCE(error, '') LIKE ? ESCAPE '\\')".to_owned());
        values.push(Value::Text(pattern.clone()));
        values.push(Value::Text(pattern));
    }
    if let Some(host) = host {
        clauses.push("host = ? COLLATE NOCASE".to_owned());
        values.push(Value::Text(host.clone()));
    }
    if !methods.is_empty() {
        clauses.push(format!("method IN ({})", placeholders(methods.len())));
        for method in methods {
            values.push(Value::Text(method.as_str().to_owned()));
        }
    }
    let status_sql = status_clause(status, &mut values)?;
    if let Some(status_sql) = status_sql {
        clauses.push(status_sql);
    }
    if !media_kinds.is_empty() {
        clauses.push(format!(
            "media_kind IN ({})",
            placeholders(media_kinds.len())
        ));
        for kind in media_kinds {
            values.push(Value::Text(kind.as_str().to_owned()));
        }
    }
    if !sources.is_empty() {
        clauses.push(format!("source IN ({})", placeholders(sources.len())));
        for source in sources {
            values.push(Value::Text(source.as_str().to_owned()));
        }
    }
    if !states.is_empty() {
        clauses.push(format!("state IN ({})", placeholders(states.len())));
        for state in states {
            values.push(Value::Text(state.as_str().to_owned()));
        }
    }
    if *only_failed {
        clauses.push("state IN ('failed', 'aborted')".to_owned());
    }
    Ok((clauses.join(" AND "), values))
}

/// Builds the status predicate, honouring `StatusRange` semantics.
fn status_clause(
    status: &netra_core::domain::StatusRange,
    values: &mut Vec<Value>,
) -> StorageResult<Option<String>> {
    if status.is_empty() {
        return Ok(None);
    }
    // `StatusRange` hides its ranges; expose them through the text form so the
    // predicate stays in SQL instead of filtering in memory.
    let text = status.to_text();
    let mut parts = Vec::new();
    if let Ok(ranges) = netra_core::domain::StatusRange::parse(&text) {
        for term in ranges.to_text().split(',') {
            if let Some((from, to)) = term.split_once('-') {
                let from: i64 = from.parse().unwrap_or(0);
                let to: i64 = to.parse().unwrap_or(0);
                parts.push("status_code BETWEEN ? AND ?".to_owned());
                values.push(Value::Integer(from));
                values.push(Value::Integer(to));
            } else if term.len() == 3 && term.ends_with("xx") {
                let class: i64 = term[..1].parse().unwrap_or(0);
                parts.push("status_code BETWEEN ? AND ?".to_owned());
                values.push(Value::Integer(class * 100));
                values.push(Value::Integer(class * 100 + 99));
            } else {
                parts.push("status_code = ?".to_owned());
                values.push(Value::Integer(term.parse().unwrap_or(-1)));
            }
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("({})", parts.join(" OR "))))
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Escapes LIKE wildcards so user text is matched literally.
fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_escapes_wildcards() {
        assert_eq!(escape_like("100%_a"), "100\\%\\_a");
    }
}
