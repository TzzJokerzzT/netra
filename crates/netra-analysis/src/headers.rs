//! Header analysis: highlights worth pointing at instead of reading 40 lines.

use netra_core::domain::HttpHeaders;

/// Why a header matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderNote {
    /// Server-side caching behaviour.
    Cache,
    /// Compression.
    Encoding,
    /// Cross-origin policy.
    Cors,
    /// A transport security policy.
    Security,
    /// A cookie directive.
    Cookie,
    /// Authentication challenge.
    Auth,
    /// Retry or rate limiting hint.
    RateLimit,
    /// Redirect target.
    Redirect,
    /// Server fingerprint.
    Fingerprint,
    /// Missing security hardening.
    Weakness,
}

/// One highlighted header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderHighlight {
    /// Header name as captured.
    pub name: String,
    /// Header value as captured (already redacted by the capture pipeline).
    pub value: String,
    /// Why it is highlighted.
    pub note: HeaderNote,
    /// Short explanation for the inspector.
    pub comment: &'static str,
}

/// Headers that deserve a note, in a stable order.
#[must_use]
pub fn highlights(headers: &HttpHeaders) -> Vec<HeaderHighlight> {
    const RULES: &[(&str, HeaderNote, &str)] = &[
        (
            "cache-control",
            HeaderNote::Cache,
            "browser/CDN caching directives",
        ),
        (
            "content-encoding",
            HeaderNote::Encoding,
            "transfer compression",
        ),
        (
            "access-control-allow-origin",
            HeaderNote::Cors,
            "CORS allowlist",
        ),
        (
            "strict-transport-security",
            HeaderNote::Security,
            "HSTS policy",
        ),
        (
            "content-security-policy",
            HeaderNote::Security,
            "CSP policy",
        ),
        ("set-cookie", HeaderNote::Cookie, "cookie directives"),
        (
            "www-authenticate",
            HeaderNote::Auth,
            "authentication challenge",
        ),
        ("retry-after", HeaderNote::RateLimit, "retry hint"),
        ("location", HeaderNote::Redirect, "redirect target"),
        ("server", HeaderNote::Fingerprint, "server banner"),
        ("x-powered-by", HeaderNote::Fingerprint, "framework banner"),
    ];
    let mut found: Vec<HeaderHighlight> = Vec::new();
    for (name, note, comment) in RULES {
        for value in headers.get_all(name) {
            found.push(HeaderHighlight {
                name: (*name).to_owned(),
                value: value.to_owned(),
                note: *note,
                comment,
            });
        }
    }
    if !headers.contains("strict-transport-security") && headers.contains("content-type") {
        found.push(HeaderHighlight {
            name: "strict-transport-security".to_owned(),
            value: String::new(),
            note: HeaderNote::Weakness,
            comment: "no HSTS header on this response",
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notable_headers_are_highlighted() {
        let headers = HttpHeaders::from_pairs([
            ("Content-Type", "text/html"),
            ("Cache-Control", "no-store"),
            ("Content-Encoding", "gzip"),
            ("Set-Cookie", "a=1"),
            ("X-Custom", "ignored"),
        ]);
        let found = highlights(&headers);
        let notes: Vec<HeaderNote> = found.iter().map(|highlight| highlight.note).collect();
        assert!(notes.contains(&HeaderNote::Cache));
        assert!(notes.contains(&HeaderNote::Encoding));
        assert!(notes.contains(&HeaderNote::Cookie));
        assert!(notes.contains(&HeaderNote::Weakness));
        assert!(!found.iter().any(|highlight| highlight.name == "X-Custom"));
    }
}
