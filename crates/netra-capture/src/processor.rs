//! Correlation and normalization of capture events.
//!
//! [`CaptureProcessor`] is a pure state machine: it receives one
//! [`CaptureEvent`] and returns the incremental [`ExchangeUpdate`]s that a
//! storage adapter must apply. It keeps only correlation state (provider key to
//! exchange identifier), never payloads, so a long capture cannot grow the
//! process without bound.

use std::collections::HashMap;

use netra_core::{
    config::CaptureLimits,
    domain::{
        Body, BodyDirection, CaptureEvent, ExchangeId, ExchangeState, ExchangeUpdate,
        NetworkExchange, NetworkRequest, NetworkResponse, OmitReason, RedactionPolicy, SessionId,
        SourceKey, TimingMetrics,
    },
};

/// Counters describing what the processor had to drop, merge or mask.
///
/// They exist so the application can surface loss instead of hiding it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessorDiagnostics {
    /// `RequestStarted` events for a key that was already known.
    pub duplicate_starts: u64,
    /// Responses seen before their request and merged later.
    pub orphan_responses: u64,
    /// Orphan responses discarded because the bounded cache was full.
    pub orphan_responses_dropped: u64,
    /// Updates that referenced an unknown exchange.
    pub unmatched_updates: u64,
    /// Header lines dropped by the configured per-side limit.
    pub headers_truncated: u64,
    /// Payloads truncated by the configured byte limit.
    pub bodies_truncated: u64,
    /// Payloads withheld because body capture is disabled.
    pub bodies_withheld: u64,
    /// Structured payloads refused because they exceeded the byte limit.
    pub bodies_omitted: u64,
    /// Header values masked by the redaction policy.
    pub headers_redacted: u64,
    /// Payload values masked by the redaction policy.
    pub payload_values_redacted: u64,
}

/// Bounded cache for responses that arrive before their request.
const MAX_ORPHANS: usize = 256;

/// Correlates provider events into exchange updates.
#[derive(Debug)]
pub struct CaptureProcessor {
    session_id: SessionId,
    limits: CaptureLimits,
    policy: RedactionPolicy,
    keys: HashMap<SourceKey, ExchangeId>,
    orphans: HashMap<SourceKey, NetworkResponse>,
    diagnostics: ProcessorDiagnostics,
}

impl CaptureProcessor {
    /// Builds a processor for one session.
    #[must_use]
    pub fn new(session_id: SessionId, limits: CaptureLimits, policy: RedactionPolicy) -> Self {
        Self {
            session_id,
            limits,
            policy,
            keys: HashMap::new(),
            orphans: HashMap::new(),
            diagnostics: ProcessorDiagnostics::default(),
        }
    }

    /// Session identifier stamped on every created exchange.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Diagnostics accumulated so far.
    #[must_use]
    pub fn diagnostics(&self) -> ProcessorDiagnostics {
        self.diagnostics
    }

    /// Number of exchanges currently tracked for correlation.
    #[must_use]
    pub fn tracked_exchanges(&self) -> usize {
        self.keys.len()
    }

    /// Applies one capture event, returning the updates to persist in order.
    pub fn apply(&mut self, event: CaptureEvent) -> Vec<ExchangeUpdate> {
        match event {
            CaptureEvent::RequestStarted { key, request } => self.on_request_started(key, request),
            CaptureEvent::RequestRedirected {
                key,
                previous,
                request,
            } => self.on_redirect(key, previous, request),
            CaptureEvent::ResponseReceived { key, response } => self.on_response(key, response),
            CaptureEvent::BodyUpdated {
                key,
                direction,
                body,
            } => self.on_body(key, direction, body),
            CaptureEvent::RequestCompleted {
                key,
                timing,
                response,
            } => self.on_completed(key, timing, response),
            CaptureEvent::RequestFailed { key, error, timing } => {
                self.on_failed(key, error, timing)
            }
            CaptureEvent::RequestAborted { key, reason } => self.on_aborted(key, reason),
            CaptureEvent::SourceDisconnected { .. } | CaptureEvent::SourceReconnected { .. } => {
                // Lifecycle notices are handled by the application layer; they
                // never create or mutate an exchange.
                Vec::new()
            }
        }
    }

    fn on_request_started(
        &mut self,
        key: SourceKey,
        request: NetworkRequest,
    ) -> Vec<ExchangeUpdate> {
        if self.keys.contains_key(&key) {
            self.diagnostics.duplicate_starts += 1;
            return Vec::new();
        }
        let orphan = self.orphans.remove(&key);
        let request = self.sanitize_request(request);
        let mut exchange = NetworkExchange::pending(
            self.session_id,
            key.source,
            request,
            Some(key.request_id.clone()),
        );
        if let Some(response) = orphan {
            exchange.complete(response);
        }
        self.keys.insert(key, exchange.id);
        vec![ExchangeUpdate::Discovered(Box::new(exchange))]
    }

    fn on_redirect(
        &mut self,
        key: SourceKey,
        previous: NetworkResponse,
        request: NetworkRequest,
    ) -> Vec<ExchangeUpdate> {
        let previous_id = self.keys.get(&key).copied();
        let previous = self.sanitize_response(previous);
        let request = self.sanitize_request(request);
        let next = NetworkExchange::pending(
            self.session_id,
            key.source,
            request,
            Some(key.request_id.clone()),
        );
        let next_id = next.id;
        self.keys.insert(key, next_id);
        match previous_id {
            Some(previous_id) => vec![ExchangeUpdate::Redirected {
                previous_id,
                previous: Box::new(previous),
                next: Box::new(next),
            }],
            None => {
                // The redirect response belongs to a request we never saw.
                self.diagnostics.unmatched_updates += 1;
                vec![ExchangeUpdate::Discovered(Box::new(next))]
            }
        }
    }

    fn on_response(&mut self, key: SourceKey, response: NetworkResponse) -> Vec<ExchangeUpdate> {
        let response = self.sanitize_response(response);
        match self.keys.get(&key) {
            Some(id) => vec![ExchangeUpdate::Response {
                id: *id,
                response: Box::new(response),
            }],
            None => {
                self.diagnostics.orphan_responses += 1;
                if self.orphans.len() >= MAX_ORPHANS {
                    self.diagnostics.orphan_responses_dropped += 1;
                    return Vec::new();
                }
                self.orphans.insert(key, response);
                Vec::new()
            }
        }
    }

    fn on_body(
        &mut self,
        key: SourceKey,
        direction: BodyDirection,
        body: Body,
    ) -> Vec<ExchangeUpdate> {
        let Some(id) = self.keys.get(&key).copied() else {
            self.diagnostics.unmatched_updates += 1;
            return Vec::new();
        };
        let body = self.sanitize_body(body);
        vec![ExchangeUpdate::Body {
            id,
            direction,
            body: Box::new(body),
        }]
    }

    fn on_completed(
        &mut self,
        key: SourceKey,
        timing: Option<TimingMetrics>,
        response: Option<NetworkResponse>,
    ) -> Vec<ExchangeUpdate> {
        let Some(id) = self.keys.get(&key).copied() else {
            self.diagnostics.unmatched_updates += 1;
            return Vec::new();
        };
        let mut updates = Vec::with_capacity(3);
        if let Some(response) = response {
            updates.push(ExchangeUpdate::Response {
                id,
                response: Box::new(self.sanitize_response(response)),
            });
        }
        if let Some(timing) = timing.filter(TimingMetrics::has_any) {
            updates.push(ExchangeUpdate::Timing { id, timing });
        }
        updates.push(ExchangeUpdate::State {
            id,
            state: ExchangeState::Completed,
            error: None,
        });
        updates
    }

    fn on_failed(
        &mut self,
        key: SourceKey,
        error: String,
        timing: Option<TimingMetrics>,
    ) -> Vec<ExchangeUpdate> {
        let Some(id) = self.keys.get(&key).copied() else {
            self.diagnostics.unmatched_updates += 1;
            return Vec::new();
        };
        let mut updates = Vec::with_capacity(2);
        if let Some(timing) = timing.filter(TimingMetrics::has_any) {
            updates.push(ExchangeUpdate::Timing { id, timing });
        }
        updates.push(ExchangeUpdate::State {
            id,
            state: ExchangeState::Failed,
            error: Some(error),
        });
        updates
    }

    fn on_aborted(&mut self, key: SourceKey, reason: Option<String>) -> Vec<ExchangeUpdate> {
        let Some(id) = self.keys.get(&key).copied() else {
            self.diagnostics.unmatched_updates += 1;
            return Vec::new();
        };
        vec![ExchangeUpdate::State {
            id,
            state: ExchangeState::Aborted,
            error: reason,
        }]
    }

    fn sanitize_request(&mut self, mut request: NetworkRequest) -> NetworkRequest {
        request.headers = self.sanitize_headers(std::mem::take(&mut request.headers));
        if let Some(content_type) = request.headers.get("content-type") {
            request.body.content_type = Some(netra_core::domain::ContentType::parse(content_type));
        }
        request.url = self.policy.redact_url(
            &request.url,
            &mut netra_core::domain::RedactionReport::default(),
        );
        request.body = self.sanitize_body(request.body);
        request
    }

    fn sanitize_response(&mut self, mut response: NetworkResponse) -> NetworkResponse {
        response.headers = self.sanitize_headers(std::mem::take(&mut response.headers));
        if let Some(content_type) = response.headers.get("content-type") {
            response.body.content_type = Some(netra_core::domain::ContentType::parse(content_type));
        }
        response.body = self.sanitize_body(response.body);
        response
    }

    fn sanitize_headers(
        &mut self,
        headers: netra_core::domain::HttpHeaders,
    ) -> netra_core::domain::HttpHeaders {
        let limit = self.limits.max_headers_per_side;
        let mut kept = netra_core::domain::HttpHeaders::new();
        for (index, entry) in headers.iter().enumerate() {
            if index >= limit {
                self.diagnostics.headers_truncated += 1;
                continue;
            }
            kept.push(entry.name.clone(), entry.value.clone());
        }
        self.diagnostics.headers_redacted +=
            u64::try_from(self.policy.redact_headers(&mut kept)).unwrap_or(u64::MAX);
        kept
    }

    fn sanitize_body(&mut self, mut body: Body) -> Body {
        if body.is_absent() {
            return body;
        }
        if !self.policy.capture_bodies {
            if body.bytes().is_some() {
                body.redact();
                self.diagnostics.bodies_withheld += 1;
            }
            return body;
        }
        // A truncated structured payload is worse than no payload: it is not
        // usable and its prefix can still carry a secret that redaction cannot
        // find once the document stops parsing.
        if let Some(bytes) = body.bytes()
            && bytes.len() > self.limits.max_body_bytes
            && self.looks_structured(&body)
        {
            let size = body
                .size_bytes
                .max(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            let mut omitted = Body::omitted(body.content_type.clone(), OmitReason::TooLarge, size);
            omitted.encoding.clone_from(&body.encoding);
            self.diagnostics.bodies_omitted += 1;
            return omitted;
        }
        let mut report = netra_core::domain::RedactionReport::default();
        self.policy.redact_body(&mut body, &mut report);
        self.diagnostics.payload_values_redacted +=
            u64::try_from(report.json_values).unwrap_or(u64::MAX);
        if body.apply_limit(self.limits.max_body_bytes) {
            self.diagnostics.bodies_truncated += 1;
        }
        body
    }

    /// `true` when the payload is JSON or XML and therefore must never be
    /// persisted truncated.
    fn looks_structured(&self, body: &Body) -> bool {
        if let Some(content_type) = body.content_type.as_ref() {
            return content_type.is_json() || content_type.is_xml();
        }
        body.bytes().is_some_and(|bytes| {
            let head = String::from_utf8_lossy(&bytes[..bytes.len().min(64)]);
            let trimmed = head.trim_start();
            trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.starts_with("<?xml")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netra_core::domain::{
        CaptureSource, ContentType, HttpHeaders, HttpMethod, MediaKind, RequestUrl, StatusCode,
        Timestamp,
    };

    fn limits() -> CaptureLimits {
        CaptureLimits {
            max_body_bytes: 16,
            max_headers_per_side: 4,
            max_exchanges_per_session: 100,
        }
    }

    fn processor() -> CaptureProcessor {
        CaptureProcessor::new(SessionId::new(), limits(), RedactionPolicy::default())
    }

    /// Processor with room for the payloads a test inspects verbatim.
    fn roomy_processor() -> CaptureProcessor {
        let limits = CaptureLimits {
            max_body_bytes: 4096,
            ..limits()
        };
        CaptureProcessor::new(SessionId::new(), limits, RedactionPolicy::default())
    }

    fn request(url: &str) -> NetworkRequest {
        let mut request = NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new(url),
            Timestamp::from_unix_millis(1_000),
        );
        request.headers = HttpHeaders::from_pairs([("Accept", "application/json")]);
        request
    }

    fn response(status: u16) -> NetworkResponse {
        let mut response = NetworkResponse::new(
            StatusCode::try_new(status).unwrap(),
            Timestamp::from_unix_millis(1_100),
        );
        response.headers = HttpHeaders::from_pairs([("Content-Type", "application/json")]);
        response.body = Body::captured(
            Some(ContentType::parse("application/json")),
            br#"{"ok":true}"#.to_vec(),
        );
        response
    }

    fn key(id: &str) -> SourceKey {
        SourceKey::new(CaptureSource::Cdp, id)
    }

    fn discovered(update: &ExchangeUpdate) -> &NetworkExchange {
        match update {
            ExchangeUpdate::Discovered(exchange) => exchange,
            other => panic!("expected Discovered, got {other:?}"),
        }
    }

    #[test]
    fn request_is_published_pending_before_any_response() {
        let mut processor = processor();
        let updates = processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request: request("https://api.test/items"),
        });
        let exchange = discovered(&updates[0]);
        assert_eq!(exchange.state, ExchangeState::Pending);
        assert_eq!(exchange.session_id, processor.session_id());
        assert_eq!(exchange.source_request_id.as_deref(), Some("1"));
    }

    #[test]
    fn matched_response_updates_the_same_exchange() {
        let mut processor = processor();
        let first = processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request: request("https://api.test/items"),
        });
        let id = discovered(&first[0]).id;
        let updates = processor.apply(CaptureEvent::ResponseReceived {
            key: key("1"),
            response: response(200),
        });
        match &updates[0] {
            ExchangeUpdate::Response {
                id: target,
                response,
            } => {
                assert_eq!(*target, id);
                assert_eq!(response.status.as_u16(), 200);
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn out_of_order_response_is_merged_with_the_late_request() {
        let mut processor = processor();
        let none = processor.apply(CaptureEvent::ResponseReceived {
            key: key("7"),
            response: response(201),
        });
        assert!(none.is_empty());
        assert_eq!(processor.diagnostics().orphan_responses, 1);

        let updates = processor.apply(CaptureEvent::RequestStarted {
            key: key("7"),
            request: request("https://api.test/late"),
        });
        let exchange = discovered(&updates[0]);
        assert_eq!(exchange.state, ExchangeState::Completed);
        assert_eq!(exchange.status().unwrap().as_u16(), 201);
    }

    #[test]
    fn duplicate_request_start_is_counted_not_duplicated() {
        let mut processor = processor();
        let event = CaptureEvent::RequestStarted {
            key: key("1"),
            request: request("https://api.test/a"),
        };
        let first = processor.apply(event.clone());
        assert_eq!(first.len(), 1);
        let second = processor.apply(event);
        assert!(second.is_empty());
        assert_eq!(processor.diagnostics().duplicate_starts, 1);
        assert_eq!(processor.tracked_exchanges(), 1);
    }

    #[test]
    fn redirect_closes_the_previous_exchange_and_opens_a_new_one() {
        let mut processor = processor();
        let first = processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request: request("http://api.test/old"),
        });
        let previous_id = discovered(&first[0]).id;
        let updates = processor.apply(CaptureEvent::RequestRedirected {
            key: key("1"),
            previous: response(302),
            request: request("https://api.test/new"),
        });
        match &updates[0] {
            ExchangeUpdate::Redirected {
                previous_id: closed,
                previous,
                next,
            } => {
                assert_eq!(*closed, previous_id);
                assert_eq!(previous.status.as_u16(), 302);
                assert_eq!(next.request.url.as_str(), "https://api.test/new");
                assert_ne!(next.id, previous_id);
            }
            other => panic!("expected Redirected, got {other:?}"),
        }
    }

    #[test]
    fn completion_emits_timing_and_state_in_order() {
        let mut processor = processor();
        processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request: request("https://api.test/a"),
        });
        let updates = processor.apply(CaptureEvent::RequestCompleted {
            key: key("1"),
            timing: Some(TimingMetrics {
                total: Some(netra_core::domain::DurationMs::from_millis(42)),
                ..TimingMetrics::default()
            }),
            response: Some(response(200)),
        });
        assert!(matches!(updates[0], ExchangeUpdate::Response { .. }));
        assert!(matches!(updates[1], ExchangeUpdate::Timing { .. }));
        assert!(matches!(
            updates[2],
            ExchangeUpdate::State {
                state: ExchangeState::Completed,
                ..
            }
        ));

        let failed = processor.apply(CaptureEvent::RequestFailed {
            key: key("1"),
            error: "net::ERR_CONNECTION_RESET".to_owned(),
            timing: None,
        });
        assert!(matches!(
            failed[0],
            ExchangeUpdate::State {
                state: ExchangeState::Failed,
                ref error,
                ..
            } if error.as_deref() == Some("net::ERR_CONNECTION_RESET")
        ));
    }

    #[test]
    fn updates_for_unknown_exchanges_are_counted_not_invented() {
        let mut processor = processor();
        let updates = processor.apply(CaptureEvent::BodyUpdated {
            key: key("ghost"),
            direction: BodyDirection::Response,
            body: Body::captured(None, b"x".to_vec()),
        });
        assert!(updates.is_empty());
        assert_eq!(processor.diagnostics().unmatched_updates, 1);
    }

    #[test]
    fn payloads_are_truncated_and_flagged() {
        let mut processor = processor();
        let mut big = response(200);
        big.headers = HttpHeaders::from_pairs([("Content-Type", "text/plain")]);
        big.body = Body::captured(Some(ContentType::parse("text/plain")), vec![b'a'; 64]);
        processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request: request("https://api.test/a"),
        });
        let updates = processor.apply(CaptureEvent::ResponseReceived {
            key: key("1"),
            response: big,
        });
        match &updates[0] {
            ExchangeUpdate::Response { response, .. } => {
                assert!(response.body.truncated);
                assert_eq!(response.body.bytes().unwrap().len(), 16);
                assert_eq!(response.body.size_bytes, 64);
            }
            other => panic!("expected Response, got {other:?}"),
        }
        assert_eq!(processor.diagnostics().bodies_truncated, 1);
    }

    #[test]
    fn oversized_structured_payloads_are_omitted_not_truncated() {
        let mut processor = processor();
        let mut response = response(200);
        response.body = Body::captured(
            Some(ContentType::parse("application/json")),
            format!(r#"{{"token":"{}"}}"#, "a".repeat(64)).into_bytes(),
        );
        processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request: request("https://api.test/a"),
        });
        let updates = processor.apply(CaptureEvent::ResponseReceived {
            key: key("1"),
            response,
        });
        match &updates[0] {
            ExchangeUpdate::Response { response, .. } => {
                assert!(response.body.bytes().is_none());
                assert!(response.body.describe().contains("too large"));
                assert!(response.body.size_bytes > 64);
            }
            other => panic!("expected Response, got {other:?}"),
        }
        assert_eq!(processor.diagnostics().bodies_omitted, 1);
        assert_eq!(processor.diagnostics().bodies_truncated, 0);
    }

    #[test]
    fn header_limits_drop_extra_lines_and_are_reported() {
        let mut processor = processor();
        let mut request = request("https://api.test/a");
        request.headers = HttpHeaders::from_pairs([
            ("a", "1"),
            ("b", "2"),
            ("c", "3"),
            ("d", "4"),
            ("e", "5"),
            ("f", "6"),
        ]);
        let updates = processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request,
        });
        let exchange = discovered(&updates[0]);
        assert_eq!(exchange.request.headers.len(), 4);
        assert_eq!(processor.diagnostics().headers_truncated, 2);
    }

    #[test]
    fn redaction_is_applied_before_storage() {
        let mut processor = processor();
        let mut request = request("https://api.test/login?token=secret");
        request.headers.push("Authorization", "Bearer abc");
        let updates = processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request,
        });
        let exchange = discovered(&updates[0]);
        assert_eq!(
            exchange.request.headers.get("authorization"),
            Some(RedactionPolicy::default().replacement.as_str())
        );
        assert!(!exchange.request.url.as_str().contains("secret"));
        assert!(processor.diagnostics().headers_redacted >= 1);
    }

    #[test]
    fn json_payload_secrets_are_masked_in_place() {
        let mut processor = roomy_processor();
        let mut payload = Body::captured(
            Some(ContentType::parse("application/json")),
            br#"{"user":"ana","password":"hunter2"}"#.to_vec(),
        );
        payload.apply_limit(64);
        let mut response = response(200);
        response.body = payload;
        processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request: request("https://api.test/a"),
        });
        let updates = processor.apply(CaptureEvent::ResponseReceived {
            key: key("1"),
            response,
        });
        match &updates[0] {
            ExchangeUpdate::Response { response, .. } => {
                let text = response.body.text().unwrap();
                assert!(!text.contains("hunter2"));
                assert!(text.contains("ana"));
                assert!(text.starts_with("{\"user\""));
            }
            other => panic!("expected Response, got {other:?}"),
        }
        assert_eq!(processor.diagnostics().payload_values_redacted, 1);
    }

    #[test]
    fn response_content_type_is_inferred_from_headers_when_missing() {
        let mut processor = processor();
        processor.apply(CaptureEvent::RequestStarted {
            key: key("1"),
            request: request("https://api.test/a"),
        });
        let mut response = response(200);
        response.body = Body::captured(None, b"{}".to_vec());
        let updates = processor.apply(CaptureEvent::ResponseReceived {
            key: key("1"),
            response,
        });
        match &updates[0] {
            ExchangeUpdate::Response { response, .. } => {
                assert_eq!(
                    response.body.content_type.as_ref().unwrap().kind(),
                    MediaKind::Json
                );
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn source_lifecycle_events_never_touch_exchanges() {
        let mut processor = processor();
        assert!(
            processor
                .apply(CaptureEvent::SourceDisconnected {
                    source: CaptureSource::Cdp,
                    reason: "socket closed".into(),
                })
                .is_empty()
        );
        assert_eq!(processor.tracked_exchanges(), 0);
    }
}
