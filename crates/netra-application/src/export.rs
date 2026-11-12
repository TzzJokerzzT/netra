//! HAR 1.2 export.
//!
//! Only export is implemented: importing arbitrary HAR files brings parsing,
//! size and trust problems that belong to a later milestone. The CLI must not
//! offer an import command until this module grows one.

use netra_core::domain::{Body, BodyData, CaptureSession, HttpHeaders, NetworkExchange, Timestamp};
use serde_json::{Map, Value, json};

/// Builds a HAR 1.2 document for one session.
///
/// Every entry keeps the redaction already applied at capture time: the export
/// never re-introduces a secret that the store does not have.
#[must_use]
pub fn to_har(session: &CaptureSession, exchanges: &[NetworkExchange], creator: &str) -> Value {
    let entries: Vec<Value> = exchanges
        .iter()
        .map(|exchange| entry(session, exchange))
        .collect();
    let started = exchanges
        .iter()
        .map(|exchange| exchange.request.started_at)
        .min()
        .unwrap_or(session.created_at);
    json!({
        "log": {
            "version": "1.2",
            "creator": { "name": creator, "version": env!("CARGO_PKG_VERSION") },
            "pages": [{
                "startedDateTime": started.to_rfc3339(),
                "id": session.id.to_string(),
                "title": session.name,
                "pageTimings": {}
            }],
            "entries": entries,
        }
    })
}

fn entry(session: &CaptureSession, exchange: &NetworkExchange) -> Value {
    let request = &exchange.request;
    let response = exchange.response.as_ref();
    let time = exchange
        .duration()
        .map_or(0.0, |duration| duration.as_secs_f64() * 1000.0);
    let mut entry = Map::new();
    entry.insert("pageref".to_owned(), Value::String(session.id.to_string()));
    entry.insert(
        "startedDateTime".to_owned(),
        Value::String(request.started_at.to_rfc3339()),
    );
    entry.insert("time".to_owned(), json!(time));
    entry.insert(
        "request".to_owned(),
        json!({
            "method": request.method.as_str(),
            "url": request.url.as_str(),
            "httpVersion": request.http_version.clone().unwrap_or_else(|| "HTTP/1.1".to_owned()),
            "headers": headers(&request.headers),
            "queryString": query(request.url.as_str()),
            "cookies": cookies(&request.headers, "cookie"),
            "headersSize": -1,
            "bodySize": request.body.size_bytes,
            "postData": post_data(&request.body),
        }),
    );
    entry.insert(
        "response".to_owned(),
        match response {
            Some(response) => json!({
                "status": response.status.as_u16(),
                "statusText": response.status_text.clone().unwrap_or_default(),
                "httpVersion": response.http_version.clone().unwrap_or_else(|| "HTTP/1.1".to_owned()),
                "headers": headers(&response.headers),
                "cookies": cookies(&response.headers, "set-cookie"),
                "content": content(&response.body),
                "redirectURL": response.headers.get("location").unwrap_or_default(),
                "headersSize": -1,
                "bodySize": response.body.size_bytes,
            }),
            None => json!({
                "status": 0,
                "statusText": "",
                "httpVersion": "",
                "headers": [],
                "cookies": [],
                "content": { "size": 0, "mimeType": "" },
                "redirectURL": "",
                "headersSize": -1,
                "bodySize": -1,
            }),
        },
    );
    entry.insert("cache".to_owned(), json!({}));
    if let Some(timing) = exchange.timing {
        entry.insert(
            "timings".to_owned(),
            json!({
                "blocked": harvest(timing.blocked),
                "dns": harvest(timing.dns),
                "connect": harvest(timing.connect),
                "ssl": harvest(timing.tls),
                "send": 0,
                "wait": harvest(timing.ttfb),
                "receive": harvest(timing.download),
            }),
        );
    }
    if let Some(error) = exchange.error.as_ref() {
        entry.insert("_error".to_owned(), Value::String(error.clone()));
    }
    entry.insert(
        "_netra".to_owned(),
        json!({
            "id": exchange.id.to_string(),
            "source": exchange.source.as_str(),
            "state": exchange.state.as_str(),
            "mediaKind": exchange.media_kind().as_str(),
        }),
    );
    Value::Object(entry)
}

fn harvest(value: Option<netra_core::domain::DurationMs>) -> f64 {
    value.map_or(-1.0, |duration| duration.as_secs_f64() * 1000.0)
}

fn headers(headers: &HttpHeaders) -> Vec<Value> {
    headers
        .iter()
        .map(|entry| json!({ "name": entry.name, "value": entry.value }))
        .collect()
}

fn query(url: &str) -> Vec<Value> {
    url::Url::parse(url).ok().map_or_else(Vec::new, |parsed| {
        parsed
            .query_pairs()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect()
    })
}

fn cookies(headers: &HttpHeaders, searched: &str) -> Vec<Value> {
    headers
        .get_all(searched)
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .map(|(name, value)| json!({ "name": name.trim(), "value": value.trim() }))
        .collect()
}

fn post_data(body: &Body) -> Value {
    match &body.data {
        BodyData::Captured { .. } => {
            let text = body.text().unwrap_or_default();
            json!({
                "mimeType": body.content_type.as_ref().map_or_else(String::new, ToString::to_string),
                "text": text,
            })
        }
        BodyData::Absent | BodyData::Redacted | BodyData::Omitted { .. } => Value::Null,
    }
}

fn content(body: &Body) -> Value {
    let mime = body
        .content_type
        .as_ref()
        .map_or_else(String::new, ToString::to_string);
    let mut content = Map::new();
    content.insert("size".to_owned(), json!(body.size_bytes));
    content.insert("mimeType".to_owned(), Value::String(mime));
    match &body.data {
        BodyData::Captured { .. } => {
            content.insert(
                "text".to_owned(),
                Value::String(body.text().unwrap_or_default()),
            );
            if body.truncated {
                content.insert("_truncated".to_owned(), Value::Bool(true));
            }
        }
        BodyData::Omitted { reason } => {
            content.insert(
                "_omitted".to_owned(),
                Value::String(reason.label().to_owned()),
            );
        }
        BodyData::Redacted => {
            content.insert("_redacted".to_owned(), Value::Bool(true));
        }
        BodyData::Absent => {}
    }
    if let Some(encoding) = body.encoding.as_ref() {
        content.insert("encoding".to_owned(), Value::String(encoding.clone()));
    }
    Value::Object(content)
}

/// Renders the document as compact JSON with a trailing newline.
///
/// # Errors
///
/// Returns a message when serialization fails.
pub fn to_har_string(
    session: &CaptureSession,
    exchanges: &[NetworkExchange],
    creator: &str,
) -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(&to_har(session, exchanges, creator))
        .map_err(|error| error.to_string())?;
    text.push('\n');
    Ok(text)
}

/// Session start instant used by exporters.
#[must_use]
pub fn session_start(session: &CaptureSession) -> Timestamp {
    session.created_at
}

#[cfg(test)]
mod tests {
    use netra_core::domain::{
        Body, CaptureSession, CaptureSource, ContentType, ExchangeState, HttpHeaders, HttpMethod,
        NetworkExchange, NetworkRequest, NetworkResponse, RequestUrl, SourceSelection, StatusCode,
        Timestamp,
    };

    use super::*;

    fn session() -> CaptureSession {
        CaptureSession::new("api", SourceSelection::Cdp, Timestamp::from_unix_millis(0)).unwrap()
    }

    fn exchange(session: &CaptureSession) -> NetworkExchange {
        let mut request = NetworkRequest::new(
            HttpMethod::parse("POST").unwrap(),
            RequestUrl::new("https://api.test/login?page=2"),
            Timestamp::from_unix_millis(1_000),
        );
        request.headers = HttpHeaders::from_pairs([("Content-Type", "application/json")]);
        request.body = Body::captured(
            Some(ContentType::parse("application/json")),
            br#"{"user":"ana"}"#.to_vec(),
        );
        let mut response = NetworkResponse::new(
            StatusCode::try_new(201).unwrap(),
            Timestamp::from_unix_millis(1_120),
        );
        response.headers = HttpHeaders::from_pairs([
            ("Content-Type", "application/json"),
            ("Set-Cookie", "sid=abc; Path=/"),
        ]);
        response.body = Body::captured(
            Some(ContentType::parse("application/json")),
            br#"{"id":1}"#.to_vec(),
        );
        let mut exchange = NetworkExchange::pending(session.id, CaptureSource::Cdp, request, None);
        exchange.complete(response);
        exchange
    }

    #[test]
    fn har_document_contains_the_entry_fields() {
        let session = session();
        let exchange = exchange(&session);
        let document = to_har(&session, std::slice::from_ref(&exchange), "netra");
        let entry = &document["log"]["entries"][0];
        assert_eq!(entry["request"]["method"], "POST");
        assert_eq!(entry["request"]["url"], "https://api.test/login?page=2");
        assert_eq!(entry["response"]["status"], 201);
        assert_eq!(entry["time"].as_f64().unwrap(), 120.0);
        assert_eq!(entry["_netra"]["source"], "cdp");
        assert_eq!(entry["request"]["postData"]["text"], "{\"user\":\"ana\"}");
        assert_eq!(
            document["log"]["entries"][0]["response"]["cookies"][0]["name"],
            "sid"
        );
    }

    #[test]
    fn pending_exchanges_are_exported_with_status_zero() {
        let session = session();
        let request = NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("https://api.test/pending"),
            Timestamp::from_unix_millis(10),
        );
        let exchange = NetworkExchange::pending(session.id, CaptureSource::Proxy, request, None);
        assert_eq!(exchange.state, ExchangeState::Pending);
        let document = to_har(&session, &[exchange], "netra");
        assert_eq!(document["log"]["entries"][0]["response"]["status"], 0);
    }

    #[test]
    fn redacted_and_omitted_bodies_are_marked() {
        let session = session();
        let mut exchange = exchange(&session);
        if let Some(response) = exchange.response.as_mut() {
            response.body = Body::redacted(Some(ContentType::parse("application/json")), 10);
        }
        let document = to_har(&session, &[exchange], "netra");
        assert_eq!(
            document["log"]["entries"][0]["response"]["content"]["_redacted"],
            true
        );
    }

    #[test]
    fn har_string_ends_with_a_newline() {
        let session = session();
        let text = to_har_string(&session, &[exchange(&session)], "netra").unwrap();
        assert!(text.ends_with('\n'));
        assert!(text.contains("\"version\": \"1.2\""));
        assert_eq!(session_start(&session), session.created_at);
    }
}
