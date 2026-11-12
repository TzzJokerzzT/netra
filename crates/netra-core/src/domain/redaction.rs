//! Redaction policy: secrets are removed before persistence and rendering.
//!
//! Redaction is applied by the capture processor, not by the UI, so no adapter
//! can bypass it. The default policy masks the usual credential carriers and
//! can be extended through configuration.

use serde::{Deserialize, Serialize};

use super::{
    body::{Body, BodyData},
    exchange::{NetworkExchange, NetworkResponse},
    headers::HttpHeaders,
    http::RequestUrl,
};

/// Marker written in place of a redacted value.
pub const REDACTION_MARKER: &str = "[REDACTED]";

/// Header names masked by default.
pub const DEFAULT_SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
    "x-csrf-token",
    "x-xsrf-token",
];

/// Query parameters masked by default.
pub const DEFAULT_SENSITIVE_QUERY_PARAMS: &[&str] = &[
    "access_token",
    "api_key",
    "apikey",
    "auth",
    "code",
    "id_token",
    "key",
    "password",
    "refresh_token",
    "secret",
    "sig",
    "signature",
    "token",
];

/// JSON fields masked by default.
pub const DEFAULT_SENSITIVE_JSON_FIELDS: &[&str] = &[
    "access_token",
    "api_key",
    "apikey",
    "authorization",
    "client_secret",
    "id_token",
    "password",
    "passwd",
    "refresh_token",
    "secret",
    "token",
];

/// Configurable redaction rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionPolicy {
    /// Lower-cased header names whose values are masked.
    #[serde(default = "default_header_names")]
    pub header_names: Vec<String>,
    /// Lower-cased query parameter names whose values are masked.
    #[serde(default = "default_query_params")]
    pub query_params: Vec<String>,
    /// Lower-cased JSON field names whose values are masked.
    #[serde(default = "default_json_fields")]
    pub json_fields: Vec<String>,
    /// Replacement text for masked values.
    #[serde(default = "default_marker")]
    pub replacement: String,
    /// When `false`, payloads are stored as [`BodyData::Redacted`].
    #[serde(default = "default_true")]
    pub capture_bodies: bool,
}

fn default_header_names() -> Vec<String> {
    DEFAULT_SENSITIVE_HEADERS
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

fn default_query_params() -> Vec<String> {
    DEFAULT_SENSITIVE_QUERY_PARAMS
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

fn default_json_fields() -> Vec<String> {
    DEFAULT_SENSITIVE_JSON_FIELDS
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

fn default_marker() -> String {
    REDACTION_MARKER.to_owned()
}

fn default_true() -> bool {
    true
}

impl Default for RedactionPolicy {
    fn default() -> Self {
        Self {
            header_names: default_header_names(),
            query_params: default_query_params(),
            json_fields: default_json_fields(),
            replacement: default_marker(),
            capture_bodies: true,
        }
    }
}

/// Counts of what a redaction pass changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RedactionReport {
    /// Number of header values masked.
    pub headers: usize,
    /// Number of URL query values masked.
    pub query_values: usize,
    /// Number of JSON values masked.
    pub json_values: usize,
    /// Number of payloads withheld entirely.
    pub bodies: usize,
}

impl RedactionReport {
    /// `true` when nothing needed masking.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.headers + self.query_values + self.json_values + self.bodies == 0
    }
}

impl RedactionPolicy {
    /// Builds a policy that masks nothing but keeps payload capture enabled.
    ///
    /// This is only for tests and for explicit opt-out configurations.
    #[must_use]
    pub fn permissive() -> Self {
        Self {
            header_names: Vec::new(),
            query_params: Vec::new(),
            json_fields: Vec::new(),
            replacement: REDACTION_MARKER.to_owned(),
            capture_bodies: true,
        }
    }

    /// `true` when the header is configured as sensitive.
    #[must_use]
    pub fn is_sensitive_header(&self, name: &str) -> bool {
        self.header_names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }

    /// Masks every configured header value in place, keeping names and order.
    pub fn redact_headers(&self, headers: &mut HttpHeaders) -> usize {
        let mut changed = 0;
        let names = self.header_names.clone();
        for name in names {
            for entry in headers.iter_mut() {
                if entry.name_is(&name) && entry.value != self.replacement {
                    entry.value.clone_from(&self.replacement);
                    changed += 1;
                }
            }
        }
        changed
    }

    /// Masks configured query parameter values in a URL.
    #[must_use]
    pub fn redact_url(&self, url: &RequestUrl, report: &mut RedactionReport) -> RequestUrl {
        let Some(parsed) = url.parsed() else {
            return url.clone();
        };
        if parsed.query().is_none() {
            return url.clone();
        }
        let mut changed = false;
        let pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(name, value)| {
                if self
                    .query_params
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&name))
                {
                    changed = true;
                    report.query_values += 1;
                    (name.into_owned(), self.replacement.clone())
                } else {
                    (name.into_owned(), value.into_owned())
                }
            })
            .collect();
        if !changed {
            return url.clone();
        }
        let mut rebuilt = parsed.clone();
        {
            let mut serializer = rebuilt.query_pairs_mut();
            serializer.clear();
            for (name, value) in pairs {
                serializer.append_pair(&name, &value);
            }
        }
        RequestUrl::new(rebuilt.to_string())
    }

    /// Recursively masks configured field names in a JSON document.
    pub fn redact_json(&self, value: &mut serde_json::Value, report: &mut RedactionReport) {
        match value {
            serde_json::Value::Object(map) => {
                for (field, entry) in map.iter_mut() {
                    if self
                        .json_fields
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(field))
                    {
                        if !entry.is_null() {
                            *entry = serde_json::Value::String(self.replacement.clone());
                            report.json_values += 1;
                        }
                    } else {
                        self.redact_json(entry, report);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    self.redact_json(item, report);
                }
            }
            _ => {}
        }
    }

    /// Applies the payload policy and content redaction to one body.
    pub fn redact_body(&self, body: &mut Body, report: &mut RedactionReport) {
        if body.is_absent() {
            return;
        }
        if !self.capture_bodies {
            if body.bytes().is_some() {
                body.redact();
                report.bodies += 1;
            }
            return;
        }
        let Some(text) = body.text() else {
            return;
        };
        let looks_json = body
            .content_type
            .as_ref()
            .is_some_and(super::http::ContentType::is_json)
            || text.trim_start().starts_with('{')
            || text.trim_start().starts_with('[');
        if !looks_json {
            return;
        }
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let before = report.json_values;
        self.redact_json(&mut value, report);
        if report.json_values != before
            && let Ok(serialized) = serde_json::to_vec(&value)
        {
            let len = u64::try_from(serialized.len()).unwrap_or(u64::MAX);
            body.size_bytes = body.size_bytes.max(len);
            body.data = BodyData::Captured { bytes: serialized };
        }
    }

    /// Applies every rule to one exchange and reports what was masked.
    pub fn apply(&self, exchange: &mut NetworkExchange) -> RedactionReport {
        let mut report = RedactionReport::default();
        let redacted_url = self.redact_url(&exchange.request.url, &mut report);
        exchange.request.url = redacted_url;
        report.headers += self.redact_headers(&mut exchange.request.headers);
        self.redact_body(&mut exchange.request.body, &mut report);
        if let Some(response) = exchange.response.as_mut() {
            Self::redact_response(self, response, &mut report);
        }
        report
    }

    fn redact_response(&self, response: &mut NetworkResponse, report: &mut RedactionReport) {
        report.headers += self.redact_headers(&mut response.headers);
        self.redact_body(&mut response.body, report);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        Body, CaptureSource, ContentType, ExchangeState, HttpHeaders, HttpMethod, NetworkRequest,
        NetworkResponse, SessionId, StatusCode, Timestamp,
    };

    fn exchange_with_secrets() -> NetworkExchange {
        let mut request = NetworkRequest::new(
            HttpMethod::parse("POST").unwrap(),
            RequestUrl::new("https://api.test/login?token=abc&page=2"),
            Timestamp::from_unix_millis(0),
        );
        request.headers = HttpHeaders::from_pairs([
            ("Authorization", "Bearer super-secret"),
            ("Accept", "application/json"),
            ("Cookie", "session=xyz"),
        ]);
        request.body = Body::captured(
            Some(ContentType::parse("application/json")),
            br#"{"user":"ana","password":"hunter2","nested":{"token":"t"}}"#.to_vec(),
        );
        let mut response = NetworkResponse::new(
            StatusCode::try_new(200).unwrap(),
            Timestamp::from_unix_millis(15),
        );
        response.headers = HttpHeaders::from_pairs([
            ("set-cookie", "sid=1"),
            ("Content-Type", "application/json"),
        ]);
        response.body = Body::captured(
            Some(ContentType::parse("application/json")),
            br#"{"access_token":"at","ok":true}"#.to_vec(),
        );
        let mut exchange = NetworkExchange::pending(
            SessionId::new(),
            CaptureSource::Cdp,
            request,
            Some("1".into()),
        );
        exchange.complete(response);
        exchange
    }

    #[test]
    fn default_policy_masks_the_documented_headers() {
        let policy = RedactionPolicy::default();
        for name in [
            "Authorization",
            "Proxy-Authorization",
            "Cookie",
            "Set-Cookie",
        ] {
            assert!(policy.is_sensitive_header(name), "{name} must be masked");
        }
        assert!(!policy.is_sensitive_header("Accept"));
    }

    #[test]
    fn headers_keep_names_and_order_but_lose_values() {
        let policy = RedactionPolicy::default();
        let mut headers = HttpHeaders::from_pairs([
            ("Authorization", "Bearer x"),
            ("Accept", "application/json"),
            ("authorization", "Bearer y"),
        ]);
        assert_eq!(policy.redact_headers(&mut headers), 2);
        assert_eq!(
            headers.get_all("authorization").collect::<Vec<_>>(),
            vec![REDACTION_MARKER, REDACTION_MARKER]
        );
        assert_eq!(headers.get("Accept"), Some("application/json"));
        assert_eq!(headers.iter().next().unwrap().name, "Authorization");
    }

    #[test]
    fn query_secrets_are_masked_and_others_kept() {
        let policy = RedactionPolicy::default();
        let mut report = RedactionReport::default();
        let url = policy.redact_url(
            &RequestUrl::new("https://api.test/x?token=abc&page=2"),
            &mut report,
        );
        assert!(
            url.as_str().contains("token=%5BREDACTED%5D")
                || url.as_str().contains("token=[REDACTED]")
        );
        assert!(url.as_str().contains("page=2"));
        assert_eq!(report.query_values, 1);
    }

    #[test]
    fn json_fields_are_masked_recursively() {
        let policy = RedactionPolicy::default();
        let mut value = serde_json::json!({
            "user": "ana",
            "password": "hunter2",
            "nested": { "token": "t", "keep": 1 },
            "list": [{ "secret": "s" }],
        });
        let mut report = RedactionReport::default();
        policy.redact_json(&mut value, &mut report);
        assert_eq!(value["user"], "ana");
        assert_eq!(value["password"], REDACTION_MARKER);
        assert_eq!(value["nested"]["token"], REDACTION_MARKER);
        assert_eq!(value["nested"]["keep"], 1);
        assert_eq!(value["list"][0]["secret"], REDACTION_MARKER);
        assert_eq!(report.json_values, 3);
    }

    #[test]
    fn full_pass_redacts_headers_query_and_json_in_both_directions() {
        let policy = RedactionPolicy::default();
        let mut exchange = exchange_with_secrets();
        let report = policy.apply(&mut exchange);
        assert_eq!(report.headers, 3);
        assert_eq!(report.query_values, 1);
        // password + nested token in the request, access_token in the response
        assert_eq!(report.json_values, 3);
        let request_text = exchange.request.body.text().unwrap();
        assert!(!request_text.contains("hunter2"));
        assert!(request_text.contains("ana"));
        let response_text = exchange.response.as_ref().unwrap().body.text().unwrap();
        assert!(!response_text.contains("\"at\""));
        assert_eq!(exchange.state, ExchangeState::Completed);
    }

    #[test]
    fn disabled_body_capture_withholds_payloads() {
        let policy = RedactionPolicy {
            capture_bodies: false,
            ..RedactionPolicy::default()
        };
        let mut exchange = exchange_with_secrets();
        let report = policy.apply(&mut exchange);
        assert!(report.bodies >= 1);
        assert!(exchange.request.body.bytes().is_none());
        assert!(exchange.request.body.size_bytes > 0);
    }

    #[test]
    fn binary_payloads_are_left_untouched() {
        let policy = RedactionPolicy::default();
        let mut body = Body::captured(
            Some(ContentType::parse("image/png")),
            vec![0x89, 0x50, 0x4e],
        );
        let mut report = RedactionReport::default();
        policy.redact_body(&mut body, &mut report);
        assert_eq!(body.bytes().unwrap(), &[0x89, 0x50, 0x4e]);
        assert!(report.is_empty());
    }

    #[test]
    fn malformed_json_payloads_are_not_rewritten() {
        let policy = RedactionPolicy::default();
        let mut body = Body::captured(
            Some(ContentType::parse("application/json")),
            b"{\"token\": ".to_vec(),
        );
        let mut report = RedactionReport::default();
        policy.redact_body(&mut body, &mut report);
        assert_eq!(body.bytes().unwrap(), b"{\"token\": ");
    }
}
