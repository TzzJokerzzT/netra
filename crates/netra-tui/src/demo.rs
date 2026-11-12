//! Synthetic demo data for the render/keymap tests and the CLI `--demo` mode.
//!
//! **This is not a real capture.** Every request, header, body and timing below
//! is fabricated so the screen can be exercised without a browser, a proxy or a
//! database. Anything built from this module must be labelled as demo data and
//! must never be presented as captured traffic.

use netra_core::domain::{
    Body, CaptureSource, ContentType, DurationMs, ExchangeId, ExchangeSummary, HttpHeaders,
    HttpMethod, NetworkExchange, NetworkRequest, NetworkResponse, RequestUrl, SessionId,
    StatusCode, Timestamp, TimingMetrics, redaction::REDACTION_MARKER,
};

use crate::app::{InspectorData, SessionHeader, Snapshot};

/// Wall-clock instant the synthetic session starts at.
const BASE_MS: i64 = 1_700_000_000_000;

/// Snapshot of the synthetic session: header plus five exchange rows.
#[must_use]
pub fn snapshot() -> Snapshot {
    let rows: Vec<ExchangeSummary> = exchanges().iter().map(NetworkExchange::summary).collect();
    let header = header(&rows);
    Snapshot { header, rows }
}

/// Full details of the same five synthetic exchanges, in snapshot order.
#[must_use]
pub fn details() -> Vec<InspectorData> {
    exchanges()
        .into_iter()
        .map(|exchange| InspectorData { exchange })
        .collect()
}

/// Header describing the synthetic session.
fn header(rows: &[ExchangeSummary]) -> SessionHeader {
    let errors = rows
        .iter()
        .filter(|row| {
            matches!(
                row.state,
                netra_core::domain::ExchangeState::Failed
                    | netra_core::domain::ExchangeState::Aborted
            ) || row.status.is_some_and(StatusCode::is_error)
        })
        .count();
    SessionHeader {
        session_id: Some(demo_session_id()),
        name: "demo session".to_owned(),
        status: "capturing".to_owned(),
        sources: "both".to_owned(),
        total: u64::try_from(rows.len()).unwrap_or(u64::MAX),
        errors: u64::try_from(errors).unwrap_or(u64::MAX),
        elapsed: DurationMs::from_millis(65_000),
    }
}

/// The five synthetic exchanges.
fn exchanges() -> Vec<NetworkExchange> {
    [
        completed_json_get(),
        pending_post(),
        server_error(),
        failed_transport(),
        redacted_body(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Deterministic session identifier so `snapshot()` and `details()` agree.
fn demo_session_id() -> SessionId {
    SessionId::parse("11111111-1111-4111-8111-111111111111").unwrap_or_default()
}

/// Deterministic exchange identifier so `snapshot()` and `details()` agree.
fn demo_exchange_id(index: u8) -> ExchangeId {
    let text = format!("00000000-0000-4000-8000-0000000000{index:02}");
    ExchangeId::parse(&text).unwrap_or_default()
}

/// A completed JSON `GET` with phase timings.
fn completed_json_get() -> Option<NetworkExchange> {
    let method = HttpMethod::parse("GET").ok()?;
    let started = Timestamp::from_unix_millis(BASE_MS);
    let mut request = NetworkRequest::new(
        method,
        RequestUrl::new("https://api.example.com/v1/products?page=2&limit=20"),
        started,
    );
    request.headers = HttpHeaders::from_pairs([
        ("Accept", "application/json"),
        ("User-Agent", "netra-demo/0.1"),
    ]);
    request.http_version = Some("HTTP/2".to_owned());
    let mut exchange = NetworkExchange::pending(
        demo_session_id(),
        CaptureSource::Cdp,
        request,
        Some("demo-1".to_owned()),
    );
    exchange.id = demo_exchange_id(1);

    let mut response = NetworkResponse::new(
        StatusCode::try_new(200).ok()?,
        Timestamp::from_unix_millis(BASE_MS + 247),
    );
    response.status_text = Some("OK".to_owned());
    response.headers = HttpHeaders::from_pairs([
        ("Content-Type", "application/json; charset=utf-8"),
        ("Content-Encoding", "gzip"),
        ("Cache-Control", "no-store"),
    ]);
    response.body = Body::captured(
        Some(ContentType::parse("application/json; charset=utf-8")),
        br#"{"items":[{"id":1,"name":"Desk lamp"},{"id":2,"name":"Chair"}],"total":120}"#.to_vec(),
    );
    response.remote_address = Some("203.0.113.10:443".to_owned());
    response.http_version = Some("HTTP/2".to_owned());
    exchange.complete(response);
    exchange.set_timing(TimingMetrics {
        blocked: Some(DurationMs::from_millis(5)),
        dns: Some(DurationMs::from_millis(12)),
        connect: Some(DurationMs::from_millis(30)),
        tls: Some(DurationMs::from_millis(45)),
        ttfb: Some(DurationMs::from_millis(120)),
        download: Some(DurationMs::from_millis(35)),
        total: Some(DurationMs::from_millis(247)),
    });
    Some(exchange)
}

/// A request that is still pending.
fn pending_post() -> Option<NetworkExchange> {
    let method = HttpMethod::parse("POST").ok()?;
    let mut request = NetworkRequest::new(
        method,
        RequestUrl::new("https://api.example.com/v1/orders"),
        Timestamp::from_unix_millis(BASE_MS + 300),
    );
    request.headers = HttpHeaders::from_pairs([
        ("Content-Type", "application/json"),
        ("Accept", "application/json"),
    ]);
    request.body = Body::captured(
        Some(ContentType::parse("application/json")),
        br#"{"sku":"NT-100","quantity":2}"#.to_vec(),
    );
    let mut exchange = NetworkExchange::pending(
        demo_session_id(),
        CaptureSource::Proxy,
        request,
        Some("demo-2".to_owned()),
    );
    exchange.id = demo_exchange_id(2);
    Some(exchange)
}

/// A completed `500` response with an HTML error page.
fn server_error() -> Option<NetworkExchange> {
    let method = HttpMethod::parse("GET").ok()?;
    let started = Timestamp::from_unix_millis(BASE_MS + 500);
    let request = NetworkRequest::new(
        method,
        RequestUrl::new("https://api.example.com/v1/users/42"),
        started,
    );
    let mut exchange = NetworkExchange::pending(
        demo_session_id(),
        CaptureSource::Proxy,
        request,
        Some("demo-3".to_owned()),
    );
    exchange.id = demo_exchange_id(3);

    let mut response = NetworkResponse::new(
        StatusCode::try_new(500).ok()?,
        Timestamp::from_unix_millis(BASE_MS + 500 + 812),
    );
    response.status_text = Some("Internal Server Error".to_owned());
    response.headers = HttpHeaders::from_pairs([
        ("Content-Type", "text/html; charset=utf-8"),
        ("Cache-Control", "no-store"),
    ]);
    response.body = Body::captured(
        Some(ContentType::parse("text/html; charset=utf-8")),
        b"<html><body><h1>Internal Server Error</h1></body></html>".to_vec(),
    );
    exchange.complete(response);
    exchange.set_timing(TimingMetrics {
        blocked: Some(DurationMs::from_millis(2)),
        dns: Some(DurationMs::from_millis(8)),
        connect: Some(DurationMs::from_millis(20)),
        ttfb: Some(DurationMs::from_millis(780)),
        download: Some(DurationMs::from_millis(2)),
        total: Some(DurationMs::from_millis(812)),
        ..TimingMetrics::default()
    });
    Some(exchange)
}

/// A transport failure with no response at all.
fn failed_transport() -> Option<NetworkExchange> {
    let method = HttpMethod::parse("GET").ok()?;
    let request = NetworkRequest::new(
        method,
        RequestUrl::new("https://cdn.example.net/assets/logo.png"),
        Timestamp::from_unix_millis(BASE_MS + 900),
    );
    let mut exchange = NetworkExchange::pending(
        demo_session_id(),
        CaptureSource::Cdp,
        request,
        Some("demo-4".to_owned()),
    );
    exchange.id = demo_exchange_id(4);
    exchange.fail("net::ERR_CONNECTION_REFUSED");
    Some(exchange)
}

/// A completed exchange whose credentials and body were withheld by policy.
fn redacted_body() -> Option<NetworkExchange> {
    let method = HttpMethod::parse("GET").ok()?;
    let started = Timestamp::from_unix_millis(BASE_MS + 1_200);
    let mut request = NetworkRequest::new(
        method,
        RequestUrl::new("https://api.example.com/v1/me"),
        started,
    );
    request.headers = HttpHeaders::from_pairs([
        ("Accept", "application/json"),
        ("Authorization", REDACTION_MARKER),
        ("Cookie", REDACTION_MARKER),
    ]);
    let mut exchange = NetworkExchange::pending(
        demo_session_id(),
        CaptureSource::Cdp,
        request,
        Some("demo-5".to_owned()),
    );
    exchange.id = demo_exchange_id(5);

    let mut response = NetworkResponse::new(
        StatusCode::try_new(200).ok()?,
        Timestamp::from_unix_millis(BASE_MS + 1_200 + 96),
    );
    response.status_text = Some("OK".to_owned());
    response.headers = HttpHeaders::from_pairs([
        ("Content-Type", "application/json; charset=utf-8"),
        ("Set-Cookie", REDACTION_MARKER),
    ]);
    response.body = Body::redacted(
        Some(ContentType::parse("application/json; charset=utf-8")),
        512,
    );
    exchange.complete(response);
    exchange.set_timing(TimingMetrics {
        dns: Some(DurationMs::from_millis(4)),
        connect: Some(DurationMs::from_millis(18)),
        ttfb: Some(DurationMs::from_millis(70)),
        download: Some(DurationMs::from_millis(4)),
        total: Some(DurationMs::from_millis(96)),
        ..TimingMetrics::default()
    });
    Some(exchange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use netra_core::domain::ExchangeState;

    #[test]
    fn snapshot_and_details_share_identifiers() {
        let snapshot = snapshot();
        let details = details();
        assert_eq!(snapshot.rows.len(), 5);
        assert_eq!(details.len(), 5);
        let row_ids: Vec<ExchangeId> = snapshot.rows.iter().map(|row| row.id).collect();
        let detail_ids: Vec<ExchangeId> = details.iter().map(|detail| detail.exchange.id).collect();
        assert_eq!(row_ids, detail_ids);
    }

    #[test]
    fn demo_covers_the_required_scenarios() {
        let details = details();
        assert!(
            details.iter().any(|detail| {
                detail.exchange.state == ExchangeState::Completed
                    && detail.exchange.status().is_some_and(StatusCode::is_success)
                    && detail.exchange.media_kind() == netra_core::domain::MediaKind::Json
                    && detail.exchange.request.method.as_str() == "GET"
            }),
            "missing completed JSON GET"
        );
        assert!(
            details.iter().any(|detail| {
                detail.exchange.state == ExchangeState::Pending
                    && detail.exchange.request.method.as_str() == "POST"
            }),
            "missing pending POST"
        );
        assert!(
            details.iter().any(|detail| detail
                .exchange
                .status()
                .is_some_and(|status| status.as_u16() == 500)),
            "missing 500 response"
        );
        assert!(
            details
                .iter()
                .any(|detail| detail.exchange.state == ExchangeState::Failed),
            "missing transport failure"
        );
        assert!(
            details.iter().any(|detail| detail
                .exchange
                .response
                .as_ref()
                .is_some_and(|response| response.body.bytes().is_none())),
            "missing redacted body"
        );
    }

    #[test]
    fn header_counts_derive_from_the_rows() {
        let snapshot = snapshot();
        assert_eq!(snapshot.header.total, 5);
        assert_eq!(snapshot.header.errors, 2);
        assert_eq!(snapshot.header.status, "capturing");
        assert_eq!(snapshot.header.sources, "both");
        assert!(snapshot.header.session_id.is_some());
    }
}
