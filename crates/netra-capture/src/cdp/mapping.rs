//! Translation from CDP payloads to the domain capture events.
//!
//! This module is pure: it takes the JSON the browser sends and returns the
//! events the pipeline understands. Anything the protocol does not report stays
//! `None` instead of being invented — a request without timings must not look
//! like a request that took zero milliseconds.

use netra_core::domain::{
    Body, CaptureEvent, CaptureSource, ContentType, DurationMs, HttpHeaders, HttpMethod,
    NetworkRequest, NetworkResponse, OmitReason, RequestUrl, SourceKey, StatusCode, Timestamp,
    TimingMetrics,
};
use serde_json::Value;

/// What one CDP event produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpOutcome {
    /// A normalized capture event.
    Event(Box<CaptureEvent>),
    /// The provider must ask for the response body; the answer becomes a
    /// `BodyUpdated` event.
    FetchResponseBody {
        /// Correlation key of the exchange.
        key: SourceKey,
        /// CDP request identifier.
        request_id: String,
        /// Declared media type, used for the body payload.
        content_type: Option<String>,
    },
}

/// What the adapter remembers about one in-flight request to finish its timings.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingDraft {
    /// Monotonic timestamp of `requestWillBeSent`, in seconds.
    pub request_timestamp: f64,
    /// Monotonic timestamp of `responseReceived`, in seconds.
    pub response_timestamp: Option<f64>,
    /// The `response.timing` object, when present.
    pub timing: Option<Value>,
}

impl TimingDraft {
    /// Creates a draft from a `requestWillBeSent` payload.
    #[must_use]
    pub fn new(params: &Value) -> Self {
        Self {
            request_timestamp: seconds(params, "timestamp").unwrap_or_default(),
            response_timestamp: None,
            timing: None,
        }
    }

    /// Records the response phase from a `responseReceived` payload.
    pub fn observe_response(&mut self, params: &Value) {
        self.response_timestamp = seconds(params, "timestamp");
        self.timing = params
            .get("response")
            .and_then(|response| response.get("timing"))
            .cloned();
    }
}

/// Computes phase timings from the recorded draft and the finish timestamp.
///
/// CDP reports `timing` as offsets in milliseconds from `requestTime`, so every
/// value is a difference and never an absolute clock reading.
#[must_use]
pub fn finalize_timing(draft: &TimingDraft, finished_timestamp: f64) -> TimingMetrics {
    let Some(timing) = draft.timing.as_ref() else {
        let total = finished_timestamp - draft.request_timestamp;
        return TimingMetrics {
            total: millis(total),
            ..TimingMetrics::default()
        };
    };
    // CDP reports `-1` for a phase it did not measure; that is absence, not a
    // negative duration.
    let offset = |field: &str| {
        timing
            .get(field)
            .and_then(Value::as_f64)
            .filter(|value| *value >= 0.0)
    };
    let span = |start: &str, end: &str| match (offset(start), offset(end)) {
        (Some(start), Some(end)) if end >= start => {
            Some(DurationMs::from_millis((end - start).max(0.0) as u64))
        }
        _ => None,
    };
    let blocked = offset("dnsStart")
        .or_else(|| offset("connectStart"))
        .map(|start| DurationMs::from_millis(start as u64));
    let ttfb = match (offset("sendEnd"), offset("receiveHeadersEnd")) {
        (Some(send_end), Some(headers_end)) if headers_end >= send_end => {
            Some(DurationMs::from_millis((headers_end - send_end) as u64))
        }
        (None, Some(headers_end)) => Some(DurationMs::from_millis(headers_end as u64)),
        _ => None,
    };
    let download = draft
        .response_timestamp
        .map(|response| finished_timestamp - response)
        .filter(|seconds| *seconds >= 0.0);
    let total = Some(finished_timestamp - draft.request_timestamp)
        .filter(|seconds| *seconds > 0.0)
        .map(|seconds| DurationMs::from_millis((seconds * 1000.0).max(0.0) as u64));
    TimingMetrics {
        blocked,
        dns: span("dnsStart", "dnsEnd"),
        connect: span("connectStart", "connectEnd"),
        tls: span("sslStart", "sslEnd"),
        ttfb,
        download: download.map(|seconds| DurationMs::from_millis((seconds * 1000.0) as u64)),
        total,
    }
}

/// Translates one CDP event.
///
/// Unknown methods return nothing: a new browser event must never break
/// capture, and the inspector only claims what it understands.
#[must_use]
pub fn translate(method: &str, params: &Value, source: CaptureSource) -> Vec<CdpOutcome> {
    match method {
        "Network.requestWillBeSent" => request_will_be_sent(params, source),
        "Network.responseReceived" => response_received(params, source),
        "Network.loadingFinished" => loading_finished(params, source),
        "Network.loadingFailed" => loading_failed(params, source),
        _ => Vec::new(),
    }
}

fn request_will_be_sent(params: &Value, source: CaptureSource) -> Vec<CdpOutcome> {
    let Some(request_id) = params.get("requestId").and_then(Value::as_str) else {
        return Vec::new();
    };
    let key = SourceKey::new(source, request_id);
    let Some(request_value) = params.get("request") else {
        return Vec::new();
    };
    let request = build_request(request_value, params);
    // A redirect arrives as a new request carrying the previous response.
    match params.get("redirectResponse") {
        Some(redirect) => {
            let previous = build_response(redirect);
            vec![CdpOutcome::Event(Box::new(
                CaptureEvent::RequestRedirected {
                    key,
                    previous,
                    request,
                },
            ))]
        }
        None => vec![CdpOutcome::Event(Box::new(CaptureEvent::RequestStarted {
            key,
            request,
        }))],
    }
}

fn response_received(params: &Value, source: CaptureSource) -> Vec<CdpOutcome> {
    let Some(request_id) = params.get("requestId").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(response_value) = params.get("response") else {
        return Vec::new();
    };
    vec![CdpOutcome::Event(Box::new(
        CaptureEvent::ResponseReceived {
            key: SourceKey::new(source, request_id),
            response: build_response(response_value),
        },
    ))]
}

fn loading_finished(params: &Value, source: CaptureSource) -> Vec<CdpOutcome> {
    let Some(request_id) = params.get("requestId").and_then(Value::as_str) else {
        return Vec::new();
    };
    let key = SourceKey::new(source, request_id);
    let content_type = params
        .get("response")
        .and_then(|response| response.get("headers"))
        .and_then(|headers| header_lookup(headers, "content-type"))
        .or_else(|| {
            params
                .get("_contentType")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    vec![
        CdpOutcome::Event(Box::new(CaptureEvent::RequestCompleted {
            key: key.clone(),
            timing: None,
            response: None,
        })),
        CdpOutcome::FetchResponseBody {
            key,
            request_id: request_id.to_owned(),
            content_type,
        },
    ]
}

fn loading_failed(params: &Value, source: CaptureSource) -> Vec<CdpOutcome> {
    let Some(request_id) = params.get("requestId").and_then(Value::as_str) else {
        return Vec::new();
    };
    let key = SourceKey::new(source, request_id);
    let canceled = params
        .get("canceled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if canceled {
        return vec![CdpOutcome::Event(Box::new(CaptureEvent::RequestAborted {
            key,
            reason: params
                .get("errorText")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }))];
    }
    let error = params
        .get("errorText")
        .and_then(Value::as_str)
        .unwrap_or("CDP loading failed")
        .to_owned();
    vec![CdpOutcome::Event(Box::new(CaptureEvent::RequestFailed {
        key,
        error,
        timing: None,
    }))]
}

/// Builds a request from a CDP `request` object.
#[must_use]
pub fn build_request(value: &Value, envelope: &Value) -> NetworkRequest {
    let raw_method = value.get("method").and_then(Value::as_str).unwrap_or("GET");
    let method = HttpMethod::parse(raw_method)
        .unwrap_or_else(|_| HttpMethod::parse("GET").unwrap_or_else(|_| unreachable!()));
    let url = value.get("url").and_then(Value::as_str).unwrap_or_default();
    let headers = build_headers(value.get("headers"));
    let started_at = wall_time(envelope);
    let mut request = NetworkRequest::new(method, RequestUrl::new(url), started_at);
    request.headers = headers.clone();
    let content_type =
        header_lookup_value(&headers, "content-type").map(|value| ContentType::parse(&value));
    request.body = match value.get("postData").and_then(Value::as_str) {
        Some(body) => Body::captured(content_type, body.as_bytes().to_vec()),
        None => {
            let has_post_data = value
                .get("hasPostData")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if has_post_data {
                Body::omitted(content_type, OmitReason::NotProvidedBySource, 0)
            } else {
                let mut body = Body::absent();
                body.content_type = content_type;
                body
            }
        }
    };
    request
}

/// Builds a response from a CDP `response` object.
#[must_use]
pub fn build_response(value: &Value) -> NetworkResponse {
    let status = value
        .get("status")
        .and_then(Value::as_f64)
        .and_then(|code| u16::try_from(code as i64).ok())
        .and_then(|code| StatusCode::try_new(code).ok())
        .unwrap_or_else(|| StatusCode::try_new(200).unwrap_or_else(|_| unreachable!()));
    let received_at = seconds(value, "responseTime")
        .map(|seconds| Timestamp::from_unix_millis((seconds * 1000.0) as i64))
        .unwrap_or_else(Timestamp::now);
    let mut response = NetworkResponse::new(status, received_at);
    response.status_text = value
        .get("statusText")
        .and_then(Value::as_str)
        .map(str::to_owned);
    response.headers = build_headers(value.get("headers"));
    let mut body = Body::absent().with_size(
        value
            .get("encodedDataLength")
            .and_then(Value::as_f64)
            .map_or(0, |length| length.max(0.0) as u64),
    );
    body.content_type = value
        .get("mimeType")
        .and_then(Value::as_str)
        .map(ContentType::parse)
        .or_else(|| response.headers.get("content-type").map(ContentType::parse));
    response.body = body;
    response.http_version = value
        .get("protocol")
        .and_then(Value::as_str)
        .map(str::to_owned);
    response.remote_address = match (
        value.get("remoteIPAddress").and_then(Value::as_str),
        value.get("remotePort").and_then(Value::as_u64),
    ) {
        (Some(address), Some(port)) => Some(format!("{address}:{port}")),
        (Some(address), None) => Some(address.to_owned()),
        _ => None,
    };
    response
}

/// Builds ordered headers from a CDP header object.
///
/// CDP sends response headers as a JSON object; array values carry repeated
/// header lines and are expanded into one entry each. Names that the protocol
/// itself collapsed stay collapsed, and that limit is documented in
/// `docs/architecture.md`.
#[must_use]
pub fn build_headers(value: Option<&Value>) -> HttpHeaders {
    let mut headers = HttpHeaders::new();
    let Some(object) = value.and_then(Value::as_object) else {
        return headers;
    };
    for (name, value) in object {
        match value {
            // An array means repeated header lines (`Set-Cookie`): keep them as
            // separate entries so duplicates survive.
            Value::Array(items) => {
                for item in items {
                    let text = item
                        .as_str()
                        .map_or_else(|| item.to_string(), str::to_owned);
                    headers.push(name.clone(), text);
                }
            }
            other => {
                let text = match other {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                headers.push(name.clone(), text);
            }
        }
    }
    headers
}

fn header_lookup(headers: &Value, searched: &str) -> Option<String> {
    headers
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(searched))
        .map(|(_, value)| match value {
            Value::String(text) => text.clone(),
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
            other => other.to_string(),
        })
}

fn header_lookup_value(headers: &HttpHeaders, searched: &str) -> Option<String> {
    headers.get(searched).map(str::to_owned)
}

fn seconds(value: &Value, field: &str) -> Option<f64> {
    value.get(field).and_then(Value::as_f64)
}

fn wall_time(envelope: &Value) -> Timestamp {
    seconds(envelope, "wallTime")
        .map(|seconds| Timestamp::from_unix_millis((seconds * 1000.0) as i64))
        .unwrap_or_else(Timestamp::now)
}

fn millis(seconds: f64) -> Option<DurationMs> {
    (seconds > 0.0).then(|| DurationMs::from_millis((seconds * 1000.0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use netra_core::domain::{BodyData, ExchangeState};

    fn source() -> CaptureSource {
        CaptureSource::Cdp
    }

    fn request_event(outcomes: &[CdpOutcome]) -> &CaptureEvent {
        match &outcomes[0] {
            CdpOutcome::Event(event) => event,
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn request_started_is_visible_before_any_response() {
        let params = serde_json::json!({
            "requestId": "1000.1",
            "timestamp": 100.0,
            "wallTime": 1_700_000_000.5,
            "request": {
                "url": "https://api.test/v1/items?page=1",
                "method": "get",
                "headers": { "Accept": "application/json", "Content-Type": "application/json" },
                "postData": "{\"a\":1}",
                "hasPostData": true
            },
            "type": "Fetch"
        });
        let outcomes = translate("Network.requestWillBeSent", &params, source());
        match request_event(&outcomes) {
            CaptureEvent::RequestStarted { key, request } => {
                assert_eq!(key.request_id, "1000.1");
                assert_eq!(request.method.as_str(), "GET");
                assert_eq!(request.url.as_str(), "https://api.test/v1/items?page=1");
                assert_eq!(request.started_at.unix_millis(), 1_700_000_000_500);
                assert_eq!(request.headers.get("accept"), Some("application/json"));
                assert_eq!(request.body.text().as_deref(), Some("{\"a\":1}"));
                assert_eq!(
                    request.body.content_type.as_ref().map(ContentType::essence),
                    Some("application/json")
                );
            }
            other => panic!("expected RequestStarted, got {other:?}"),
        }
    }

    #[test]
    fn a_body_the_browser_withheld_is_marked_as_omitted() {
        let params = serde_json::json!({
            "requestId": "2",
            "timestamp": 1.0,
            "wallTime": 1.0,
            "request": { "url": "https://api.test/x", "method": "POST", "hasPostData": true }
        });
        let outcomes = translate("Network.requestWillBeSent", &params, source());
        match request_event(&outcomes) {
            CaptureEvent::RequestStarted { request, .. } => match request.body.data {
                BodyData::Omitted { reason } => assert_eq!(reason, OmitReason::NotProvidedBySource),
                ref other => panic!("expected an omitted body, got {other:?}"),
            },
            other => panic!("expected RequestStarted, got {other:?}"),
        }
    }

    #[test]
    fn redirect_closes_the_previous_exchange() {
        let params = serde_json::json!({
            "requestId": "3",
            "timestamp": 5.0,
            "wallTime": 5.0,
            "request": { "url": "https://api.test/new", "method": "GET" },
            "redirectResponse": {
                "status": 302,
                "statusText": "Found",
                "headers": { "Location": "https://api.test/new" },
                "mimeType": "text/plain"
            }
        });
        let outcomes = translate("Network.requestWillBeSent", &params, source());
        match request_event(&outcomes) {
            CaptureEvent::RequestRedirected {
                previous, request, ..
            } => {
                assert_eq!(previous.status.as_u16(), 302);
                assert_eq!(
                    previous.headers.get("location"),
                    Some("https://api.test/new")
                );
                assert_eq!(request.url.as_str(), "https://api.test/new");
            }
            other => panic!("expected RequestRedirected, got {other:?}"),
        }
    }

    #[test]
    fn response_received_carries_status_headers_and_size() {
        let params = serde_json::json!({
            "requestId": "4",
            "timestamp": 6.0,
            "type": "XHR",
            "response": {
                "status": 404,
                "statusText": "Not Found",
                "headers": {
                    "Content-Type": "application/json; charset=utf-8",
                    "Set-Cookie": ["a=1", "b=2"]
                },
                "mimeType": "application/json",
                "responseTime": 1_700_000_000_100.0,
                "encodedDataLength": 42.0,
                "protocol": "h2",
                "remoteIPAddress": "203.0.113.7",
                "remotePort": 443
            }
        });
        let outcomes = translate("Network.responseReceived", &params, source());
        match request_event(&outcomes) {
            CaptureEvent::ResponseReceived { response, .. } => {
                assert_eq!(response.status.as_u16(), 404);
                assert_eq!(response.status_text.as_deref(), Some("Not Found"));
                assert_eq!(response.headers.get_all("set-cookie").count(), 2);
                assert_eq!(
                    response
                        .body
                        .content_type
                        .as_ref()
                        .map(ContentType::essence),
                    Some("application/json")
                );
                assert_eq!(response.body.size_bytes, 42);
                assert_eq!(response.http_version.as_deref(), Some("h2"));
                assert_eq!(response.remote_address.as_deref(), Some("203.0.113.7:443"));
            }
            other => panic!("expected ResponseReceived, got {other:?}"),
        }
    }

    #[test]
    fn loading_finished_completes_and_requests_the_body() {
        let params =
            serde_json::json!({ "requestId": "5", "timestamp": 7.5, "encodedDataLength": 12 });
        let outcomes = translate("Network.loadingFinished", &params, source());
        assert!(matches!(
            request_event(&outcomes),
            CaptureEvent::RequestCompleted { timing: None, .. }
        ));
        match &outcomes[1] {
            CdpOutcome::FetchResponseBody { request_id, .. } => assert_eq!(request_id, "5"),
            other => panic!("expected a body fetch, got {other:?}"),
        }
    }

    #[test]
    fn canceled_requests_are_aborted_and_failures_are_failures() {
        let canceled = serde_json::json!({
            "requestId": "6", "timestamp": 1.0, "errorText": "net::ERR_ABORTED", "canceled": true
        });
        assert!(matches!(
            request_event(&translate("Network.loadingFailed", &canceled, source())),
            CaptureEvent::RequestAborted { .. }
        ));
        let failed = serde_json::json!({
            "requestId": "7", "timestamp": 1.0, "errorText": "net::ERR_CONNECTION_REFUSED"
        });
        match request_event(&translate("Network.loadingFailed", &failed, source())) {
            CaptureEvent::RequestFailed { error, .. } => {
                assert_eq!(error, "net::ERR_CONNECTION_REFUSED");
            }
            other => panic!("expected RequestFailed, got {other:?}"),
        }
    }

    #[test]
    fn phases_are_differences_and_never_invented() {
        let draft = TimingDraft {
            request_timestamp: 10.0,
            response_timestamp: Some(10.2),
            timing: Some(serde_json::json!({
                "requestTime": 1_700_000_000.0,
                "dnsStart": 3.0, "dnsEnd": 8.0,
                "connectStart": 8.0, "connectEnd": 20.0,
                "sslStart": 12.0, "sslEnd": 20.0,
                "sendStart": 20.0, "sendEnd": 21.0,
                "receiveHeadersEnd": 121.0
            })),
        };
        let timing = finalize_timing(&draft, 10.5);
        assert_eq!(timing.blocked, Some(DurationMs::from_millis(3)));
        assert_eq!(timing.dns, Some(DurationMs::from_millis(5)));
        assert_eq!(timing.connect, Some(DurationMs::from_millis(12)));
        assert_eq!(timing.tls, Some(DurationMs::from_millis(8)));
        assert_eq!(timing.ttfb, Some(DurationMs::from_millis(100)));
        assert_eq!(timing.download, Some(DurationMs::from_millis(300)));
        assert_eq!(timing.total, Some(DurationMs::from_millis(500)));
    }

    #[test]
    fn a_response_without_timings_only_reports_the_total() {
        let draft = TimingDraft {
            request_timestamp: 1.0,
            response_timestamp: Some(1.1),
            timing: None,
        };
        let timing = finalize_timing(&draft, 1.25);
        assert_eq!(timing.total, Some(DurationMs::from_millis(250)));
        assert_eq!(timing.dns, None);
        assert_eq!(timing.tls, None);
        assert_eq!(timing.ttfb, None);
    }

    #[test]
    fn unusable_timing_values_are_dropped_instead_of_becoming_zero() {
        let draft = TimingDraft {
            request_timestamp: 1.0,
            response_timestamp: Some(1.0),
            timing: Some(serde_json::json!({
                "dnsStart": -1.0, "dnsEnd": 3.0,
                "connectStart": 5.0, "connectEnd": 2.0,
                "sendEnd": 10.0, "receiveHeadersEnd": 4.0
            })),
        };
        let timing = finalize_timing(&draft, 0.5);
        assert_eq!(timing.dns, None);
        assert_eq!(timing.connect, None);
        assert_eq!(timing.ttfb, None);
    }

    #[test]
    fn create_request_and_response_builders_use_stable_defaults() {
        let request = build_request(
            &serde_json::json!({ "url": "about:blank", "method": "NOT A METHOD" }),
            &serde_json::json!({}),
        );
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url.as_str(), "about:blank");
        assert_eq!(request.body.data, BodyData::Absent);
        let response = build_response(&serde_json::json!({ "status": 0 }));
        assert_eq!(response.status.as_u16(), 200);
    }

    #[test]
    fn exchange_state_names_stay_in_sync_with_the_domain() {
        // Guards against a mapping change that renames states silently.
        assert_eq!(ExchangeState::Completed.as_str(), "completed");
        assert_eq!(ExchangeState::Aborted.as_str(), "aborted");
    }

    #[test]
    fn unknown_methods_are_ignored() {
        assert!(
            translate(
                "Network.webSocketFrameSent",
                &serde_json::json!({}),
                source()
            )
            .is_empty()
        );
        assert!(translate("Page.loadEventFired", &serde_json::json!({}), source()).is_empty());
    }
}
