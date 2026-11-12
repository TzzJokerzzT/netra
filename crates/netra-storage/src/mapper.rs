//! Row decoding and encoding between SQLite and the domain.

use netra_core::{
    domain::{
        Body, BodyData, CaptureSource, ContentType, DurationMs, ExchangeId, ExchangeState,
        ExchangeSummary, HttpHeaders, MediaKind, NetworkRequest, NetworkResponse, OmitReason,
        RequestUrl, SessionId, SessionStatus, SourceSelection, StatusCode, Timestamp,
        TimingMetrics,
    },
    error::StorageError,
};

/// Timestamp to Unix milliseconds.
pub(crate) fn millis(timestamp: Timestamp) -> i64 {
    timestamp.unix_millis()
}

/// Optional timestamp to Unix milliseconds.
pub(crate) fn opt_millis(timestamp: Option<Timestamp>) -> Option<i64> {
    timestamp.map(Timestamp::unix_millis)
}

/// Unix milliseconds to a timestamp.
pub(crate) fn timestamp(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis)
}

/// Optional Unix milliseconds to an optional timestamp.
pub(crate) fn opt_timestamp(millis: Option<i64>) -> Option<Timestamp> {
    millis.map(Timestamp::from_unix_millis)
}

/// Decodes a stored status code.
pub(crate) fn status(value: Option<i64>) -> Option<StatusCode> {
    value
        .and_then(|code| u16::try_from(code).ok())
        .and_then(|code| StatusCode::try_new(code).ok())
}

/// Decodes an optional duration.
pub(crate) fn duration(value: Option<i64>) -> Option<DurationMs> {
    value
        .and_then(|millis| u64::try_from(millis).ok())
        .map(DurationMs::from_millis)
}

/// Decodes a stored state.
pub(crate) fn state(text: &str) -> Result<ExchangeState, StorageError> {
    ExchangeState::parse(text)
        .map_err(|bad| StorageError::Decode(format!("unknown exchange state in database: {bad}")))
}

/// Decodes a stored source.
pub(crate) fn source(text: &str) -> Result<CaptureSource, StorageError> {
    CaptureSource::parse(text)
        .map_err(|bad| StorageError::Decode(format!("unknown capture source: {bad}")))
}

/// Decodes a stored session status.
pub(crate) fn session_status(text: &str) -> Result<SessionStatus, StorageError> {
    SessionStatus::parse(text)
        .map_err(|bad| StorageError::Decode(format!("unknown session status: {bad}")))
}

/// Decodes a stored source selection.
pub(crate) fn source_selection(text: &str) -> Result<SourceSelection, StorageError> {
    SourceSelection::parse(text)
        .map_err(|bad| StorageError::Decode(format!("unknown source selection: {bad}")))
}

/// Decodes a stored media kind.
pub(crate) fn media_kind(text: &str) -> MediaKind {
    MediaKind::parse(text).unwrap_or(MediaKind::Other)
}

/// Decodes a stored identifier.
pub(crate) fn session_id(text: &str) -> Result<SessionId, StorageError> {
    SessionId::parse(text).map_err(|error| StorageError::Decode(error.to_string()))
}

/// Decodes a stored identifier.
pub(crate) fn exchange_id(text: &str) -> Result<ExchangeId, StorageError> {
    ExchangeId::parse(text).map_err(|error| StorageError::Decode(error.to_string()))
}

/// Column snapshot of one `exchanges` row.
#[derive(Debug, Clone)]
pub(crate) struct ExchangeRow {
    pub id: String,
    pub session_id: String,
    pub source_request_id: Option<String>,
    pub method: String,
    pub url: String,
    pub host: Option<String>,
    pub status_code: Option<i64>,
    pub content_type: Option<String>,
    pub media_kind: String,
    pub duration_ms: Option<i64>,
    pub request_size_bytes: i64,
    pub response_size_bytes: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub state: String,
    pub source: String,
    pub error: Option<String>,
    pub http_version: Option<String>,
    pub request_http_version: Option<String>,
}

/// Columns selected for every exchange read.
pub(crate) const EXCHANGE_COLUMNS: &str = "id, session_id, source_request_id, method, url, host, \
     status_code, content_type, media_kind, duration_ms, request_size_bytes, response_size_bytes, \
     started_at, finished_at, state, source, error, http_version, request_http_version";

impl ExchangeRow {
    /// Reads the row by column name, so the select order can evolve safely.
    pub(crate) fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            session_id: row.get("session_id")?,
            source_request_id: row.get("source_request_id")?,
            method: row.get("method")?,
            url: row.get("url")?,
            host: row.get("host")?,
            status_code: row.get("status_code")?,
            content_type: row.get("content_type")?,
            media_kind: row.get("media_kind")?,
            duration_ms: row.get("duration_ms")?,
            request_size_bytes: row.get("request_size_bytes")?,
            response_size_bytes: row.get("response_size_bytes")?,
            started_at: row.get("started_at")?,
            finished_at: row.get("finished_at")?,
            state: row.get("state")?,
            source: row.get("source")?,
            error: row.get("error")?,
            http_version: row.get("http_version")?,
            request_http_version: row.get("request_http_version")?,
        })
    }

    /// Projects the row into a list summary.
    pub(crate) fn to_summary(&self) -> Result<ExchangeSummary, StorageError> {
        Ok(ExchangeSummary {
            id: exchange_id(&self.id)?,
            session_id: session_id(&self.session_id)?,
            method: netra_core::domain::HttpMethod::parse(&self.method)
                .map_err(|error| StorageError::Decode(error.to_string()))?,
            url: RequestUrl::new(self.url.clone()),
            host: self.host.clone(),
            status: status(self.status_code),
            duration: duration(self.duration_ms),
            size_bytes: u64::try_from(self.request_size_bytes + self.response_size_bytes)
                .unwrap_or(0),
            media_kind: media_kind(&self.media_kind),
            source: source(&self.source)?,
            state: state(&self.state)?,
            started_at: timestamp(self.started_at),
            error: self.error.clone(),
        })
    }

    /// Builds the request half of the exchange. Headers and bodies are attached
    /// by the caller after loading the child rows.
    pub(crate) fn to_request(&self) -> Result<NetworkRequest, StorageError> {
        Ok(NetworkRequest {
            method: netra_core::domain::HttpMethod::parse(&self.method)
                .map_err(|error| StorageError::Decode(error.to_string()))?,
            url: RequestUrl::new(self.url.clone()),
            headers: HttpHeaders::new(),
            body: Body::absent(),
            started_at: timestamp(self.started_at),
            http_version: self.request_http_version.clone(),
        })
    }

    /// Builds the response half when a status code was stored.
    pub(crate) fn to_response(&self) -> Option<NetworkResponse> {
        let status = status(self.status_code)?;
        let mut response = NetworkResponse::new(
            status,
            timestamp(self.finished_at.unwrap_or(self.started_at)),
        );
        response.body.content_type = self.content_type.as_deref().map(ContentType::parse);
        response.http_version = self.http_version.clone();
        Some(response)
    }
}

/// Encodes a payload row.
#[derive(Debug, Clone)]
pub(crate) struct BodyRow {
    pub state: String,
    pub omit_reason: Option<String>,
    pub content_type: Option<String>,
    pub encoding: Option<String>,
    pub data: Option<Vec<u8>>,
    pub size_bytes: i64,
    pub truncated: bool,
}

impl BodyRow {
    pub(crate) fn from_body(body: &Body) -> Self {
        let (state, omit_reason, data) = match &body.data {
            BodyData::Absent => ("absent", None, None),
            BodyData::Captured { bytes } => ("captured", None, Some(bytes.clone())),
            BodyData::Omitted { reason } => {
                let reason = match reason {
                    OmitReason::TooLarge => "too_large",
                    OmitReason::NotProvidedBySource => "not_provided_by_source",
                    OmitReason::RetrievalFailed => "retrieval_failed",
                    OmitReason::Undecodable => "undecodable",
                };
                ("omitted", Some(reason), None)
            }
            BodyData::Redacted => ("redacted", None, None),
        };
        Self {
            state: state.to_owned(),
            omit_reason: omit_reason.map(str::to_owned),
            content_type: body.content_type.as_ref().map(ToString::to_string),
            encoding: body.encoding.clone(),
            data,
            size_bytes: i64::try_from(body.size_bytes).unwrap_or(i64::MAX),
            truncated: body.truncated,
        }
    }

    pub(crate) fn to_body(&self) -> Body {
        let content_type = self.content_type.as_deref().map(ContentType::parse);
        let data = match self.state.as_str() {
            "captured" => BodyData::Captured {
                bytes: self.data.clone().unwrap_or_default(),
            },
            "omitted" => BodyData::Omitted {
                reason: match self.omit_reason.as_deref() {
                    Some("too_large") => OmitReason::TooLarge,
                    Some("retrieval_failed") => OmitReason::RetrievalFailed,
                    Some("undecodable") => OmitReason::Undecodable,
                    _ => OmitReason::NotProvidedBySource,
                },
            },
            "redacted" => BodyData::Redacted,
            _ => BodyData::Absent,
        };
        Body {
            content_type,
            encoding: self.encoding.clone(),
            data,
            size_bytes: u64::try_from(self.size_bytes).unwrap_or(0),
            truncated: self.truncated,
        }
    }
}

/// Encodes timings row values.
pub(crate) fn timing_values(timing: &TimingMetrics) -> [Option<i64>; 7] {
    [
        timing
            .blocked
            .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX)),
        timing
            .dns
            .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX)),
        timing
            .connect
            .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX)),
        timing
            .tls
            .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX)),
        timing
            .ttfb
            .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX)),
        timing
            .download
            .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX)),
        timing
            .total
            .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX)),
    ]
}
