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
    "api-key",
    "authentication",
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth",
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

/// Decodes percent-encoding (twice), lowercases and drops an array suffix.
fn normalize_param_name(raw: &str) -> String {
    let trimmed = raw.trim();
    let once = percent_decode(trimmed);
    let twice = percent_decode(&once);
    let without_suffix = twice.trim_end_matches("[]").to_owned();
    without_suffix.to_ascii_lowercase()
}

/// Splits `name=value`, accepting a percent-encoded separator too.
fn split_param(pair: &str) -> (&str, Option<&str>) {
    if let Some((name, value)) = pair.split_once('=') {
        return (name, Some(value));
    }
    if let Some(index) = find_ignore_ascii_case(pair, "%3D") {
        let (name, value) = pair.split_at(index);
        return (name, Some(&value[3..]));
    }
    (pair, None)
}

fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    let haystack = haystack.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    haystack.find(&needle)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            output.push(high * 16 + low);
            index += 3;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_encode(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

/// End of the value run that starts at `start`.
///
/// A run stops at a pair separator (`&`), at a fragment (`#`), at a
/// percent-encoded separator (`%26`), or at the end of the text.
/// Offset of the next absolute-URL scheme, searched from `from`.
fn next_scheme(haystack: &str, from: usize) -> Option<usize> {
    let tail = &haystack[from..];
    tail.find("http://")
        .into_iter()
        .chain(tail.find("https://"))
        .min()
}

/// End of a URL token in prose: whitespace, or punctuation that closes it.
fn url_token_end(text: &str, start: usize) -> usize {
    let closes = |character: char| {
        character.is_whitespace()
            || matches!(
                character,
                ',' | ';' | ')' | ']' | '}' | '>' | '<' | '"' | '\'' | '|'
            )
    };
    text[start..]
        .char_indices()
        .find(|(_, character)| closes(*character))
        .map_or(text.len(), |(offset, _)| start + offset)
}

/// Byte offsets that end a query-value run: a literal `&` or `#`, and their
/// percent-encoded spelling `%26`.
///
/// Precomputing them once keeps every run lookup logarithmic. Scanning the
/// remaining value per run made hostile input quadratic even though the
/// pattern search itself was linear.
fn run_terminators(haystack: &str) -> Vec<usize> {
    let mut positions: Vec<usize> = haystack
        .bytes()
        .enumerate()
        .filter(|(_, byte)| *byte == b'&' || *byte == b'#')
        .map(|(index, _)| index)
        .collect();
    let mut cursor = 0;
    while let Some(offset) = haystack[cursor..].find("%26") {
        let index = cursor + offset;
        positions.push(index);
        cursor = index + 3;
    }
    positions.sort_unstable();
    positions
}

fn extend_case_insensitive(target: &mut Vec<String>, defaults: &[&str]) {
    for default in defaults {
        if !target.iter().any(|rule| rule.eq_ignore_ascii_case(default)) {
            target.push((*default).to_owned());
        }
    }
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

    /// Extends every rule list with the built-in credential carriers.
    ///
    /// Configuration adds rules; it does not silently drop the defaults that
    /// `docs/security.md` promises. Use [`RedactionPolicy::permissive`] for an
    /// explicit opt-out.
    #[must_use]
    pub fn merged_with_defaults(mut self) -> Self {
        extend_case_insensitive(&mut self.header_names, DEFAULT_SENSITIVE_HEADERS);
        extend_case_insensitive(&mut self.query_params, DEFAULT_SENSITIVE_QUERY_PARAMS);
        extend_case_insensitive(&mut self.json_fields, DEFAULT_SENSITIVE_JSON_FIELDS);
        self
    }

    /// `true` when the header is configured as sensitive.
    #[must_use]
    pub fn is_sensitive_header(&self, name: &str) -> bool {
        self.header_names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }

    /// Masks every configured header value in place, keeping names and order.
    ///
    /// Header values that carry a URL (`Referer`, `Location`, `Origin`) keep
    /// their text but lose sensitive query values: a token in a referrer is the
    /// same leak as a token in the request line.
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
        for entry in headers.iter_mut() {
            if let Some(masked) = self.mask_url_text(&entry.value) {
                entry.value = masked;
                changed += 1;
            }
        }
        changed
    }

    /// Masks sensitive query values inside a URL embedded in free text.
    ///
    /// Accepts absolute URLs and root-relative paths (`/cb?token=...`), because
    /// both appear verbatim in bodies and headers. Any other text is left
    /// untouched.
    ///
    /// Returns `Some(masked)` only when something actually changed, so callers
    /// can leave untouched values alone.
    #[must_use]
    pub fn mask_url_text(&self, raw: &str) -> Option<String> {
        let (head, rest) = raw.split_once('?')?;
        let looks_like_url = head.contains("://") || head.starts_with('/');
        if !looks_like_url || head.contains(char::is_whitespace) {
            return None;
        }
        let (query, tail) = match rest.split_once('#') {
            Some((query, fragment)) => (query, format!("#{fragment}")),
            None => (rest, String::new()),
        };
        self.mask_query_params(query)
            .map(|masked| format!("{head}?{masked}{tail}"))
    }

    /// Masks configured query parameter values in a URL.
    ///
    /// A captured URL is not always parseable (`https://host:99999/x?token=...`,
    /// `data:`, malformed redirects). Falling back to the raw text is not
    /// acceptable: the parse failure would then be a redaction bypass. The
    /// query string is therefore also masked textually.
    #[must_use]
    pub fn redact_url(&self, url: &RequestUrl, report: &mut RedactionReport) -> RequestUrl {
        let Some(parsed) = url.parsed() else {
            return self.redact_url_textually(url, report);
        };
        if parsed.query().is_none() {
            return url.clone();
        }
        let mut changed = false;
        let pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(name, value)| {
                if self.is_sensitive_query_param(&name) {
                    changed = true;
                    report.query_values += 1;
                    return (name.into_owned(), self.replacement.clone());
                }
                // A nested URL in a parameter carries its own credentials.
                if let Some(masked) = self.mask_nested_text(&value) {
                    changed = true;
                    report.query_values += 1;
                    return (name.into_owned(), masked);
                }
                (name.into_owned(), value.into_owned())
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

    /// Masks sensitive query values in a URL that no parser accepted.
    fn redact_url_textually(&self, url: &RequestUrl, report: &mut RedactionReport) -> RequestUrl {
        let raw = url.as_str();
        let Some((head, query)) = raw.split_once('?') else {
            return url.clone();
        };
        let (query, fragment) = match query.split_once('#') {
            Some((query, fragment)) => (query, Some(fragment)),
            None => (query, None),
        };
        let mut rebuilt = String::with_capacity(raw.len());
        rebuilt.push_str(head);
        rebuilt.push('?');
        for (index, pair) in query.split('&').enumerate() {
            if index > 0 {
                rebuilt.push('&');
            }
            match pair.split_once('=') {
                Some((name, _value)) if self.is_sensitive_query_param(name) => {
                    rebuilt.push_str(name);
                    rebuilt.push('=');
                    rebuilt.push_str(&self.replacement);
                    report.query_values += 1;
                }
                _ => rebuilt.push_str(pair),
            }
        }
        if let Some(fragment) = fragment {
            rebuilt.push('#');
            rebuilt.push_str(fragment);
        }
        RequestUrl::new(rebuilt)
    }

    /// `true` when the query parameter is configured as sensitive.
    ///
    /// The comparison decodes percent-encoding (twice, because tooling
    /// double-encodes) and ignores the `[]` array suffix, so `to%6Ben`,
    /// `token%5B%5D` and `TOKEN` all match `token`.
    #[must_use]
    pub fn is_sensitive_query_param(&self, name: &str) -> bool {
        let normalized = normalize_param_name(name);
        self.query_params
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&normalized))
    }

    /// Masks sensitive parameters in a raw query string (`a=1&token=2`).
    ///
    /// Returns `Some(masked)` only when something changed.
    #[must_use]
    pub fn mask_query_params(&self, query: &str) -> Option<String> {
        let mut changed = false;
        let masked: Vec<String> = query
            .split('&')
            .map(|pair| {
                let (name, value) = split_param(pair);
                if self.is_sensitive_query_param(name) {
                    changed = true;
                    return format!("{name}={}", self.replacement);
                }
                match value {
                    Some(value)
                        if value.contains('?')
                            || value.contains("%3F")
                            || value.contains("%3f") =>
                    {
                        match self.mask_nested_text(value) {
                            Some(masked) => {
                                changed = true;
                                format!("{name}={masked}")
                            }
                            None => pair.to_owned(),
                        }
                    }
                    _ => pair.to_owned(),
                }
            })
            .collect();
        changed.then(|| masked.join("&"))
    }

    /// Masks sensitive parameters inside a nested URL that a value contains.
    ///
    /// The input is the query value exactly as the caller holds it: decoded by
    /// `form_urlencoded` in [`RedactionPolicy::redact_url`], still encoded in
    /// [`RedactionPolicy::mask_query_params`]. Both spellings of every pattern
    /// are therefore searched, and only the credential runs are replaced so the
    /// surrounding value keeps its representation.
    ///
    /// One lowercased copy of the value is built and every pattern is searched
    /// in it, so the work is linear in the value length regardless of how many
    /// runs it carries; the output is rebuilt in a single pass.
    fn mask_nested_text(&self, value: &str) -> Option<String> {
        if value.is_empty() {
            return None;
        }
        let haystack = value.to_ascii_lowercase();
        let terminators = run_terminators(&haystack);
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for name in &self.query_params {
            let name = name.to_ascii_lowercase();
            let encoded = percent_encode(&name).to_ascii_lowercase();
            // Plain, `[]` array form, and their percent-encoded spellings.
            let patterns = [
                format!("{name}="),
                format!("{name}[]="),
                format!("{name}%3d"),
                format!("{name}%5b%5d%3d"),
                format!("{encoded}%3d"),
                format!("{encoded}="),
            ];
            for pattern in &patterns {
                // The cursor keeps the scan moving forward: masking rewrites
                // the value but leaves the pattern in front of it, so a scan
                // that restarted from the beginning would rewrite that same
                // run until the process died.
                let mut cursor = 0;
                while let Some(offset) = haystack[cursor..].find(pattern.as_str()) {
                    let start = cursor + offset + pattern.len();
                    let end = terminators
                        .get(terminators.partition_point(|position| *position < start))
                        .copied()
                        .unwrap_or(value.len());
                    cursor = end.max(start);
                    if start == end || value[start..end] == self.replacement {
                        continue;
                    }
                    runs.push((start, end));
                }
            }
        }
        if runs.is_empty() {
            return None;
        }
        runs.sort_unstable();
        let mut output = String::with_capacity(value.len());
        let mut cursor = 0;
        for (start, end) in runs {
            // A run nested inside an already masked one adds nothing.
            if start < cursor {
                continue;
            }
            output.push_str(&value[cursor..start]);
            output.push_str(&self.replacement);
            cursor = end;
        }
        output.push_str(&value[cursor..]);
        Some(output)
    }

    /// Masks sensitive parameters in absolute URLs embedded in free text: only
    /// the credential run is replaced, up to the punctuation that ends a URL.
    fn mask_urls_in_text(&self, text: &str) -> Option<String> {
        let haystack = text.to_ascii_lowercase();
        let mut output = String::with_capacity(text.len());
        let mut changed = false;
        let mut cursor = 0;
        while let Some(offset) = next_scheme(&haystack, cursor) {
            let start = cursor + offset;
            let end = url_token_end(text, start);
            output.push_str(&text[cursor..start]);
            match self.mask_url_text(&text[start..end]) {
                Some(masked) => {
                    output.push_str(&masked);
                    changed = true;
                }
                None => output.push_str(&text[start..end]),
            }
            cursor = end;
        }
        output.push_str(&text[cursor..]);
        changed.then_some(output)
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
            serde_json::Value::String(text) => {
                if let Some(masked) = self.mask_url_text(text) {
                    *text = masked;
                    report.query_values += 1;
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
        // URL-encoded forms are query strings in a body.
        if body.content_type.as_ref().is_some_and(|content_type| {
            content_type.essence() == "application/x-www-form-urlencoded"
        }) {
            if let Some(masked) = self.mask_query_params(&text)
                && let BodyData::Captured { bytes } = &mut body.data
            {
                let length = u64::try_from(masked.len()).unwrap_or(u64::MAX);
                body.size_bytes = body.size_bytes.max(length);
                *bytes = masked.into_bytes();
                report.query_values += 1;
            }
            return;
        }
        let looks_json = body
            .content_type
            .as_ref()
            .is_some_and(super::http::ContentType::is_json)
            || text.trim_start().starts_with('{')
            || text.trim_start().starts_with('[');
        // Anything that is not a form or a JSON document is still scanned for
        // absolute URLs: only the credential run inside the URL is replaced, so
        // the prose around it survives untouched.
        if !looks_json {
            if let Some(masked) = self.mask_urls_in_text(&text)
                && let BodyData::Captured { bytes } = &mut body.data
            {
                let length = u64::try_from(masked.len()).unwrap_or(u64::MAX);
                body.size_bytes = body.size_bytes.max(length);
                *bytes = masked.into_bytes();
                report.query_values += 1;
            }
            return;
        }
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let before = (report.json_values, report.query_values);
        self.redact_json(&mut value, report);
        // Both counters matter: masking a URL inside a string is a query-value
        // redaction, and the masked document still has to be written back.
        if (report.json_values, report.query_values) != before
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
    fn configured_rules_extend_instead_of_replacing_defaults() {
        // Model a config file that lists only one rule.
        let merged = RedactionPolicy {
            header_names: vec!["x-my-token".to_owned()],
            query_params: Vec::new(),
            json_fields: Vec::new(),
            ..RedactionPolicy::default()
        }
        .merged_with_defaults();
        assert!(merged.is_sensitive_header("x-my-token"));
        assert!(merged.is_sensitive_header("Authorization"));
        assert!(merged.is_sensitive_query_param("token"));
        let mut headers = HttpHeaders::from_pairs([("Authorization", "Bearer x")]);
        assert_eq!(merged.redact_headers(&mut headers), 1);
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
    fn unparsable_urls_are_masked_textually() {
        let policy = RedactionPolicy::default();
        let mut report = RedactionReport::default();
        // Port out of range: `Url::parse` refuses it, so the parser branch cannot run.
        let url = policy.redact_url(
            &RequestUrl::new("https://api.test:99999/x?token=supersecret&page=2#top"),
            &mut report,
        );
        assert!(!url.as_str().contains("supersecret"), "got {url}");
        assert!(
            url.as_str()
                .contains(&format!("token={}", REDACTION_MARKER))
        );
        assert!(url.as_str().contains("page=2"));
        assert!(url.as_str().ends_with("#top"));
        assert_eq!(report.query_values, 1);
    }

    #[test]
    fn unparsable_urls_without_sensitive_params_are_untouched() {
        let policy = RedactionPolicy::default();
        let mut report = RedactionReport::default();
        let raw = "https://api.test:99999/x?page=2&sort=asc";
        let url = policy.redact_url(&RequestUrl::new(raw), &mut report);
        assert_eq!(url.as_str(), raw);
        assert!(report.is_empty());
    }

    #[test]
    fn urls_inside_json_strings_lose_their_sensitive_query_values() {
        let policy = RedactionPolicy::default();
        let mut value = serde_json::json!({
            "path": "/api/items?token=abc&page=2",
            "absolute": "https://api.test/cb?access_token=xyz",
            "note": "no url here",
        });
        let mut report = RedactionReport::default();
        policy.redact_json(&mut value, &mut report);
        // A root-relative path that carries a token is still a URL to mask.
        assert_eq!(value["path"], "/api/items?token=[REDACTED]&page=2");
        assert_eq!(
            value["absolute"],
            "https://api.test/cb?access_token=[REDACTED]"
        );
        assert_eq!(value["note"], "no url here");
        assert_eq!(report.query_values, 2);
    }

    #[test]
    fn header_values_that_are_urls_are_masked_but_kept() {
        let policy = RedactionPolicy::default();
        let mut headers = HttpHeaders::from_pairs([
            ("Referer", "http://api.test/page?token=secret&page=2"),
            ("Location", "https://api.test/next?page=3"),
            ("X-Note", "https://api.test/x?token=secret"),
        ]);
        policy.redact_headers(&mut headers);
        assert_eq!(
            headers.get("referer"),
            Some("http://api.test/page?token=[REDACTED]&page=2")
        );
        // Untouched: no sensitive parameter.
        assert_eq!(
            headers.get("location"),
            Some("https://api.test/next?page=3")
        );
        assert_eq!(
            headers.get("x-note"),
            Some("https://api.test/x?token=[REDACTED]")
        );
    }

    #[test]
    fn query_shapes_that_used_to_slip_through_are_masked() {
        let policy = RedactionPolicy::default();
        let mut report = RedactionReport::default();
        for raw in [
            "https://api.test/x?token[]=ARR1",
            "https://api.test/x?ToKeN=MIX1",
            "https://api.test/x?to%6Ben=ENC1",
            "https://api.test/x?to%256Ben=DBL1",
            "https://api.test/x?next=https%3A%2F%2Fevil%2Fcb%3Ftoken%3DNESTEDQ",
            "https://api.test/x?next=https://evil/cb?token=NESTEDPLAIN",
        ] {
            let masked = policy.redact_url(&RequestUrl::new(raw), &mut report);
            for secret in ["ARR1", "MIX1", "ENC1", "DBL1", "NESTEDQ", "NESTEDPLAIN"] {
                assert!(
                    !masked.as_str().contains(secret),
                    "{raw} kept {secret}: {}",
                    masked.as_str()
                );
            }
        }
        assert!(report.query_values >= 6);
    }

    #[test]
    fn masking_an_already_masked_nested_url_is_a_noop() {
        let policy = RedactionPolicy::default();
        let once = policy
            .mask_query_params("next=https%3A%2F%2Fevil%2Fcb%3Ftoken%3DFIRST")
            .expect("the nested parameter is masked");
        assert!(!once.contains("FIRST"), "{once}");
        // A second pass must terminate and report no change: re-masking the
        // marker is what used to spin forever.
        assert_eq!(policy.mask_query_params(&once), None, "{once}");
    }

    #[test]
    fn root_relative_referrers_are_masked_in_headers() {
        let policy = RedactionPolicy::default();
        let mut headers =
            HttpHeaders::from_pairs([("Referer", "/cb?token=RELPATH1"), ("Api-Key", "HDRAPIKEY")]);
        policy.redact_headers(&mut headers);
        assert_eq!(headers.get("referer"), Some("/cb?token=[REDACTED]"));
        assert_eq!(headers.get("api-key"), Some(REDACTION_MARKER));
    }

    #[test]
    fn form_bodies_and_plain_text_urls_are_masked_without_touching_prose() {
        let policy = RedactionPolicy::default();
        let mut report = RedactionReport::default();
        let mut form = Body::captured(
            Some(ContentType::parse("application/x-www-form-urlencoded")),
            b"user=ana&token=FORM1&page=2".to_vec(),
        );
        policy.redact_body(&mut form, &mut report);
        let text = form.text().unwrap();
        assert_eq!(text, "user=ana&token=[REDACTED]&page=2");
        assert!(!text.contains("FORM1"), "{text}");
        assert!(text.contains("user=ana") && text.contains("page=2"));
        assert_eq!(report.query_values, 1);

        // A plain-text body keeps its prose: only the credential run inside the
        // URL is replaced, never the marks and the words after it.
        let mut plain = Body::captured(
            Some(ContentType::parse("text/plain")),
            b"see https://api.test/x?token=TXT1,for details".to_vec(),
        );
        policy.redact_body(&mut plain, &mut report);
        assert_eq!(
            plain.text().unwrap(),
            "see https://api.test/x?token=[REDACTED],for details"
        );
        assert_eq!(report.query_values, 2);

        // Text without a URL is left exactly as captured.
        let mut prose = Body::captured(
            Some(ContentType::parse("text/plain")),
            b"plain note: forget-me-not".to_vec(),
        );
        policy.redact_body(&mut prose, &mut report);
        assert_eq!(prose.text().unwrap(), "plain note: forget-me-not");
        assert_eq!(report.query_values, 2);
    }

    #[test]
    fn many_urls_in_plain_text_are_masked_linearly() {
        let policy = RedactionPolicy::default();
        let mut report = RedactionReport::default();
        let body: String = (0..5_000)
            .map(|index| format!(" see https://api.test/x?token=V{index},done;"))
            .collect();
        let mut plain = Body::captured(Some(ContentType::parse("text/plain")), body.into_bytes());
        policy.redact_body(&mut plain, &mut report);
        let text = plain.text().unwrap();
        assert!(!text.contains("V0,") && !text.contains("V4999,"));
        assert_eq!(text.matches(REDACTION_MARKER).count(), 5_000);
        assert_eq!(text.matches(",done;").count(), 5_000, "the prose was eaten");
    }

    #[test]
    fn many_repeated_nested_runs_are_masked_linearly() {
        let policy = RedactionPolicy::default();
        let runs = 20_000;
        let mut body = String::from("next=https%3A%2F%2Fevil%2Fcb%3Ftoken%3Dv0");
        for index in 1..runs {
            body.push_str(&format!("%26token%3Dv{index}"));
        }
        body.push_str("&page=2");
        let masked = policy
            .mask_query_params(&body)
            .expect("the repeated nested tokens are masked");
        assert_eq!(
            masked.matches(REDACTION_MARKER).count(),
            runs,
            "every run has to be masked"
        );
        for index in [0, 1, 9_999, runs - 1] {
            assert!(
                !masked.contains(&format!("v{index}")),
                "run {index} survived"
            );
        }
        assert!(
            masked.starts_with("next=https%3A%2F%2Fevil%2Fcb%3Ftoken%3D[REDACTED]"),
            "the nested URL prefix has to be preserved"
        );
        assert!(
            masked.contains("%26token%3D[REDACTED]"),
            "the encoded separators have to be preserved"
        );
        assert!(
            masked.ends_with("%26token%3D[REDACTED]&page=2"),
            "unrelated text has to be preserved"
        );
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
