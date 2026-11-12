//! The aggregate at the centre of the inspector: one request/response exchange.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    body::Body,
    filter::ExchangeSummary,
    headers::HttpHeaders,
    http::{ContentType, HttpMethod, MediaKind, RequestUrl, StatusCode},
    ids::{ExchangeId, SessionId},
    time::{DurationMs, Timestamp},
};

/// Where an exchange was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    /// Chrome DevTools Protocol.
    Cdp,
    /// The local HTTP/HTTPS proxy.
    Proxy,
}

impl CaptureSource {
    /// Stable database and CLI spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cdp => "cdp",
            Self::Proxy => "proxy",
        }
    }

    /// Parses the stable spelling.
    ///
    /// # Errors
    ///
    /// Returns the offending text when it is not `cdp` or `proxy`.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "cdp" => Ok(Self::Cdp),
            "proxy" => Ok(Self::Proxy),
            other => Err(other.to_owned()),
        }
    }

    /// Short badge label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cdp => "CDP",
            Self::Proxy => "PROXY",
        }
    }
}

impl fmt::Display for CaptureSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which capture mechanisms a session should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSelection {
    /// Chrome DevTools Protocol only.
    Cdp,
    /// Local proxy only.
    Proxy,
    /// Both mechanisms, correlated by the processor.
    #[default]
    Both,
}

impl SourceSelection {
    /// Stable CLI spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cdp => "cdp",
            Self::Proxy => "proxy",
            Self::Both => "both",
        }
    }

    /// Parses the stable spelling.
    ///
    /// # Errors
    ///
    /// Returns the offending text when it is not `cdp`, `proxy` or `both`.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "cdp" => Ok(Self::Cdp),
            "proxy" => Ok(Self::Proxy),
            "both" => Ok(Self::Both),
            other => Err(other.to_owned()),
        }
    }

    /// Enumerates the concrete sources this selection activates.
    #[must_use]
    pub fn sources(self) -> Vec<CaptureSource> {
        match self {
            Self::Cdp => vec![CaptureSource::Cdp],
            Self::Proxy => vec![CaptureSource::Proxy],
            Self::Both => vec![CaptureSource::Cdp, CaptureSource::Proxy],
        }
    }
}

impl fmt::Display for SourceSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle state of an exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeState {
    /// The request exists; no response has been seen yet.
    Pending,
    /// A response was received.
    Completed,
    /// The request failed (transport error, TLS error, reset, ...).
    Failed,
    /// The request was cancelled before completion.
    Aborted,
}

impl ExchangeState {
    /// Stable database and CLI spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
        }
    }

    /// Parses the stable spelling.
    ///
    /// # Errors
    ///
    /// Returns the offending text when it is not a known state.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "aborted" => Ok(Self::Aborted),
            other => Err(other.to_owned()),
        }
    }

    /// `true` unless the exchange is still pending.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

impl fmt::Display for ExchangeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Phase timings reported by a capture source.
///
/// Every field is optional: sources expose different subsets, and the inspector
/// must show only real measurements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingMetrics {
    /// Time queued/stalled before the request left the client.
    pub blocked: Option<DurationMs>,
    /// DNS resolution.
    pub dns: Option<DurationMs>,
    /// TCP connection establishment.
    pub connect: Option<DurationMs>,
    /// TLS handshake.
    pub tls: Option<DurationMs>,
    /// Time to first byte after the request was sent.
    pub ttfb: Option<DurationMs>,
    /// Body download.
    pub download: Option<DurationMs>,
    /// Total observed duration.
    pub total: Option<DurationMs>,
}

impl TimingMetrics {
    /// `true` when at least one phase or the total was measured.
    #[must_use]
    pub fn has_any(&self) -> bool {
        self.total.is_some() || self.phases().next().is_some()
    }

    /// Measured phases in waterfall order.
    pub fn phases(&self) -> impl Iterator<Item = (&'static str, DurationMs)> {
        [
            ("blocked", self.blocked),
            ("dns", self.dns),
            ("connect", self.connect),
            ("tls", self.tls),
            ("ttfb", self.ttfb),
            ("download", self.download),
        ]
        .into_iter()
        .filter_map(|(label, value)| value.map(|duration| (label, duration)))
    }

    /// Sum of the measured phases.
    #[must_use]
    pub fn sum_phases(&self) -> DurationMs {
        self.phases().fold(DurationMs::ZERO, |acc, (_, duration)| {
            acc.saturating_add(duration)
        })
    }
}

/// The request half of an exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRequest {
    /// HTTP method.
    pub method: HttpMethod,
    /// Full URL as captured.
    pub url: RequestUrl,
    /// Request headers in captured order.
    pub headers: HttpHeaders,
    /// Request payload.
    pub body: Body,
    /// When the request was observed.
    pub started_at: Timestamp,
    /// HTTP protocol version when the source reports it.
    pub http_version: Option<String>,
}

impl NetworkRequest {
    /// Builds a request with no headers and no body.
    #[must_use]
    pub fn new(method: HttpMethod, url: RequestUrl, started_at: Timestamp) -> Self {
        Self {
            method,
            url,
            headers: HttpHeaders::new(),
            body: Body::absent(),
            started_at,
            http_version: None,
        }
    }

    /// Declared media type of the payload.
    #[must_use]
    pub fn content_type(&self) -> Option<&ContentType> {
        self.body.content_type.as_ref()
    }
}

/// The response half of an exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkResponse {
    /// Status code.
    pub status: StatusCode,
    /// Reason phrase when the source reports one.
    pub status_text: Option<String>,
    /// Response headers in captured order.
    pub headers: HttpHeaders,
    /// Response payload.
    pub body: Body,
    /// When the response headers were observed.
    pub received_at: Timestamp,
    /// HTTP protocol version when the source reports it.
    pub http_version: Option<String>,
    /// Remote address (`ip:port`) when known.
    pub remote_address: Option<String>,
}

impl NetworkResponse {
    /// Builds a response with no headers and no body.
    #[must_use]
    pub fn new(status: StatusCode, received_at: Timestamp) -> Self {
        Self {
            status,
            status_text: None,
            headers: HttpHeaders::new(),
            body: Body::absent(),
            received_at,
            http_version: None,
            remote_address: None,
        }
    }

    /// Declared media type of the payload.
    #[must_use]
    pub fn content_type(&self) -> Option<&ContentType> {
        self.body.content_type.as_ref()
    }
}

/// A request plus its optional response, inside a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkExchange {
    /// Exchange identifier.
    pub id: ExchangeId,
    /// Owning session.
    pub session_id: SessionId,
    /// The request.
    pub request: NetworkRequest,
    /// The response, once observed.
    pub response: Option<NetworkResponse>,
    /// Phase timings, when measured.
    pub timing: Option<TimingMetrics>,
    /// Source that produced this exchange.
    pub source: CaptureSource,
    /// Lifecycle state.
    pub state: ExchangeState,
    /// Failure description for failed or aborted exchanges.
    pub error: Option<String>,
    /// Source-specific request identifier used for correlation.
    pub source_request_id: Option<String>,
}

impl NetworkExchange {
    /// Creates a pending exchange. The request is visible immediately, before
    /// any response exists.
    #[must_use]
    pub fn pending(
        session_id: SessionId,
        source: CaptureSource,
        request: NetworkRequest,
        source_request_id: Option<String>,
    ) -> Self {
        Self {
            id: ExchangeId::new(),
            session_id,
            request,
            response: None,
            timing: None,
            source,
            state: ExchangeState::Pending,
            error: None,
            source_request_id,
        }
    }

    /// Attaches or replaces the response and marks the exchange completed.
    ///
    /// Sources replay events out of order, so a late response legitimately
    /// replaces an earlier one; the exchange stays internally consistent.
    pub fn complete(&mut self, response: NetworkResponse) {
        self.response = Some(response);
        self.state = ExchangeState::Completed;
        self.error = None;
    }

    /// Marks the exchange as failed while keeping the request visible.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.state = ExchangeState::Failed;
        self.error = Some(error.into());
    }

    /// Marks the exchange as aborted, for example by navigation.
    pub fn abort(&mut self, reason: Option<String>) {
        self.state = ExchangeState::Aborted;
        if self.error.is_none() {
            self.error = reason;
        }
    }

    /// Sets the phase timings.
    pub fn set_timing(&mut self, timing: TimingMetrics) {
        self.timing = Some(timing);
    }

    /// Status code of the response, if any.
    #[must_use]
    pub fn status(&self) -> Option<StatusCode> {
        self.response.as_ref().map(|response| response.status)
    }

    /// Best available duration: explicit total, else request-to-response span.
    #[must_use]
    pub fn duration(&self) -> Option<DurationMs> {
        if let Some(total) = self.timing.and_then(|timing| timing.total) {
            return Some(total);
        }
        self.response
            .as_ref()
            .map(|response| self.request.started_at.elapsed_until(response.received_at))
    }

    /// Media type of the response, falling back to the request.
    #[must_use]
    pub fn content_type(&self) -> Option<&ContentType> {
        self.response
            .as_ref()
            .and_then(NetworkResponse::content_type)
            .or_else(|| self.request.content_type())
    }

    /// Coarse media classification used by filters and badges.
    #[must_use]
    pub fn media_kind(&self) -> MediaKind {
        self.content_type()
            .map_or(MediaKind::Other, ContentType::kind)
    }

    /// Request plus response payload sizes when known.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        let request = self.request.body.size_bytes;
        let response = self
            .response
            .as_ref()
            .map_or(0, |response| response.body.size_bytes);
        request.saturating_add(response)
    }

    /// Host of the request URL, if it parsed.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.request.url.host()
    }

    /// Projection used by list views, filters and paginated queries.
    #[must_use]
    pub fn summary(&self) -> ExchangeSummary {
        ExchangeSummary {
            id: self.id,
            session_id: self.session_id,
            method: self.request.method.clone(),
            url: self.request.url.clone(),
            host: self.request.url.host().map(str::to_owned),
            status: self.status(),
            duration: self.duration(),
            size_bytes: self.size_bytes(),
            media_kind: self.media_kind(),
            source: self.source,
            state: self.state,
            started_at: self.request.started_at,
            error: self.error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> NetworkRequest {
        NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("https://example.com/a"),
            Timestamp::from_unix_millis(1_000),
        )
    }

    fn exchange() -> NetworkExchange {
        NetworkExchange::pending(
            SessionId::new(),
            CaptureSource::Cdp,
            request(),
            Some("1.1".into()),
        )
    }

    #[test]
    fn exchange_starts_pending_and_visible() {
        let exchange = exchange();
        assert_eq!(exchange.state, ExchangeState::Pending);
        assert!(exchange.response.is_none());
        assert_eq!(exchange.status(), None);
        assert_eq!(exchange.summary().method.as_str(), "GET");
    }

    #[test]
    fn completing_sets_state_status_and_duration() {
        let mut exchange = exchange();
        let mut response = NetworkResponse::new(
            StatusCode::try_new(200).unwrap(),
            Timestamp::from_unix_millis(1_250),
        );
        response.body =
            Body::captured(Some(ContentType::parse("application/json")), b"{}".to_vec());
        exchange.complete(response);
        assert_eq!(exchange.state, ExchangeState::Completed);
        assert_eq!(exchange.duration(), Some(DurationMs::from_millis(250)));
        assert_eq!(exchange.media_kind(), MediaKind::Json);
        assert_eq!(exchange.size_bytes(), 2);
    }

    #[test]
    fn explicit_total_timing_wins_over_span() {
        let mut exchange = exchange();
        exchange.complete(NetworkResponse::new(
            StatusCode::try_new(204).unwrap(),
            Timestamp::from_unix_millis(5_000),
        ));
        exchange.set_timing(TimingMetrics {
            total: Some(DurationMs::from_millis(90)),
            ..TimingMetrics::default()
        });
        assert_eq!(exchange.duration(), Some(DurationMs::from_millis(90)));
    }

    #[test]
    fn failures_keep_the_request_and_expose_the_error() {
        let mut exchange = exchange();
        exchange.fail("net::ERR_CONNECTION_RESET");
        assert_eq!(exchange.state, ExchangeState::Failed);
        assert_eq!(
            exchange.summary().error.as_deref(),
            Some("net::ERR_CONNECTION_RESET")
        );
    }

    #[test]
    fn abort_does_not_overwrite_an_existing_error() {
        let mut exchange = exchange();
        exchange.fail("boom");
        exchange.abort(Some("cancelled".into()));
        assert_eq!(exchange.error.as_deref(), Some("boom"));
        assert_eq!(exchange.state, ExchangeState::Aborted);
    }

    #[test]
    fn timing_phases_are_ordered_and_partial_safe() {
        let timing = TimingMetrics {
            dns: Some(DurationMs::from_millis(5)),
            ttfb: Some(DurationMs::from_millis(20)),
            ..TimingMetrics::default()
        };
        let phases: Vec<_> = timing.phases().map(|(label, _)| label).collect();
        assert_eq!(phases, vec!["dns", "ttfb"]);
        assert_eq!(timing.sum_phases(), DurationMs::from_millis(25));
        assert!(!TimingMetrics::default().has_any());
    }

    #[test]
    fn source_selection_expands_to_concrete_sources() {
        assert_eq!(
            SourceSelection::Both.sources(),
            vec![CaptureSource::Cdp, CaptureSource::Proxy]
        );
        assert_eq!(
            SourceSelection::parse("PROXY").unwrap(),
            SourceSelection::Proxy
        );
    }
}
