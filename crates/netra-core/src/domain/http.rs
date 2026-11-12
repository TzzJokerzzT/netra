//! HTTP value objects: methods, status codes, URLs and content types.

use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::DomainError;

/// An HTTP method validated against the RFC 9110 token grammar.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HttpMethod(String);

impl HttpMethod {
    /// Parses and validates an HTTP method.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidMethod`] for empty input or characters
    /// outside the RFC 9110 `token` production.
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || !trimmed.bytes().all(is_token_byte) {
            return Err(DomainError::InvalidMethod(raw.to_owned()));
        }
        Ok(Self(trimmed.to_ascii_uppercase()))
    }

    /// Returns the canonical upper-case spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Methods that must not change server state.
    #[must_use]
    pub fn is_safe(&self) -> bool {
        matches!(self.0.as_str(), "GET" | "HEAD" | "OPTIONS" | "TRACE")
    }

    /// Methods that may be retried without changing intent.
    #[must_use]
    pub fn is_idempotent(&self) -> bool {
        self.is_safe() || matches!(self.0.as_str(), "PUT" | "DELETE")
    }
}

/// Returns `true` when the byte is allowed in an HTTP token.
fn is_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    ) || byte.is_ascii_alphanumeric()
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A three-digit HTTP status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StatusCode(u16);

impl StatusCode {
    /// Builds a status code, rejecting values outside 100..=599.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidStatusCode`] for out-of-range values.
    pub const fn try_new(code: u16) -> Result<Self, DomainError> {
        if code >= 100 && code <= 599 {
            Ok(Self(code))
        } else {
            Err(DomainError::InvalidStatusCode { code })
        }
    }

    /// Returns the numeric code.
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    /// Returns the class digit (1-5).
    #[must_use]
    pub const fn class(self) -> u8 {
        (self.0 / 100) as u8
    }

    /// `2xx`
    #[must_use]
    pub const fn is_success(self) -> bool {
        self.class() == 2
    }

    /// `3xx`
    #[must_use]
    pub const fn is_redirect(self) -> bool {
        self.class() == 3
    }

    /// `4xx`
    #[must_use]
    pub const fn is_client_error(self) -> bool {
        self.class() == 4
    }

    /// `5xx`
    #[must_use]
    pub const fn is_server_error(self) -> bool {
        self.class() == 5
    }

    /// `true` for 4xx and 5xx.
    #[must_use]
    pub const fn is_error(self) -> bool {
        self.0 >= 400
    }
}

impl fmt::Display for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// A captured request URL.
///
/// Capture sources report URLs that do not always parse (`about:blank`,
/// `data:`, `blob:`, malformed redirects). The raw text is therefore always
/// preserved and the parsed form is optional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub struct RequestUrl {
    raw: String,
    #[serde(skip)]
    parsed: Option<Url>,
}

impl RequestUrl {
    /// Builds a URL while retaining the exact captured text.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let parsed = Url::parse(&raw)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https" | "ws" | "wss"));
        Self { raw, parsed }
    }

    /// Returns the exact captured text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Returns the parsed URL when the text was a valid absolute URL.
    #[must_use]
    pub fn parsed(&self) -> Option<&Url> {
        self.parsed.as_ref()
    }

    /// Returns the host, if the URL parsed.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.parsed.as_ref().and_then(Url::host_str)
    }

    /// Returns the scheme, if the URL parsed.
    #[must_use]
    pub fn scheme(&self) -> Option<&str> {
        self.parsed.as_ref().map(Url::scheme)
    }

    /// Returns path and query, if the URL parsed.
    #[must_use]
    pub fn path_and_query(&self) -> Option<String> {
        self.parsed.as_ref().map(|url| match url.query() {
            Some(query) => format!("{}?{}", url.path(), query),
            None => url.path().to_owned(),
        })
    }

    /// Returns `scheme://host[:port]`, if the URL parsed.
    #[must_use]
    pub fn origin(&self) -> Option<String> {
        let url = self.parsed.as_ref()?;
        let host = url.host_str()?;
        Some(match url.port() {
            Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
            None => format!("{}://{}", url.scheme(), host),
        })
    }

    /// Returns the path portion for display, falling back to the raw text.
    #[must_use]
    pub fn display_path(&self) -> String {
        self.path_and_query().unwrap_or_else(|| self.raw.clone())
    }
}

impl From<String> for RequestUrl {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for RequestUrl {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<RequestUrl> for String {
    fn from(value: RequestUrl) -> Self {
        value.raw
    }
}

impl fmt::Display for RequestUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// A parsed `Content-Type` header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub struct ContentType {
    raw: String,
    essence: String,
    charset: Option<String>,
}

impl ContentType {
    /// Parses a `Content-Type` value leniently.
    ///
    /// Captured traffic contains non-conforming values, so parsing never
    /// fails: an unparsable value keeps its raw text and an empty essence.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim().to_owned();
        let mut parts = raw.split(';');
        let essence = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        let charset = parts
            .filter_map(|param| param.split_once('='))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("charset"))
            .map(|(_, value)| value.trim().trim_matches('"').to_ascii_lowercase());
        Self {
            raw,
            essence,
            charset,
        }
    }

    /// Returns the lower-cased `type/subtype`.
    #[must_use]
    pub fn essence(&self) -> &str {
        &self.essence
    }

    /// Returns the lower-cased `charset` parameter, if present.
    #[must_use]
    pub fn charset(&self) -> Option<&str> {
        self.charset.as_deref()
    }

    /// Returns the exact captured header value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// `application/json` or any `+json` suffix.
    #[must_use]
    pub fn is_json(&self) -> bool {
        self.essence == "application/json" || self.essence.ends_with("+json")
    }

    /// `text/html`
    #[must_use]
    pub fn is_html(&self) -> bool {
        self.essence == "text/html"
    }

    /// XML flavours, including `+xml` suffixes.
    #[must_use]
    pub fn is_xml(&self) -> bool {
        self.essence == "application/xml"
            || self.essence == "text/xml"
            || self.essence.ends_with("+xml")
    }

    /// Any `text/*` media type.
    #[must_use]
    pub fn is_text(&self) -> bool {
        self.essence.starts_with("text/")
    }

    /// Classifies the media type for filtering, statistics and rendering.
    #[must_use]
    pub fn kind(&self) -> MediaKind {
        MediaKind::classify(&self.essence)
    }
}

impl From<String> for ContentType {
    fn from(value: String) -> Self {
        Self::parse(&value)
    }
}

impl From<ContentType> for String {
    fn from(value: ContentType) -> Self {
        value.raw
    }
}

impl fmt::Display for ContentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Coarse media classification used by filters, badges and statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// `application/json` and `+json`
    Json,
    /// XML media types
    Xml,
    /// `text/html`
    Html,
    /// CSS
    Css,
    /// JavaScript
    JavaScript,
    /// `text/*` other than HTML/CSS/JS
    Text,
    /// Images
    Image,
    /// Fonts
    Font,
    /// Forms (`application/x-www-form-urlencoded`, `multipart/form-data`)
    Form,
    /// Known binary types (archives, media, protobuf, ...)
    Binary,
    /// Anything else, including missing content types
    Other,
}

impl MediaKind {
    /// Classifies a lower-cased media essence.
    #[must_use]
    pub fn classify(essence: &str) -> Self {
        if essence.is_empty() {
            return Self::Other;
        }
        if essence == "application/json" || essence.ends_with("+json") {
            return Self::Json;
        }
        if essence == "application/xml" || essence == "text/xml" || essence.ends_with("+xml") {
            return Self::Xml;
        }
        if essence == "text/html" || essence == "application/xhtml+xml" {
            return Self::Html;
        }
        if essence == "text/css" {
            return Self::Css;
        }
        if essence == "text/javascript"
            || essence == "application/javascript"
            || essence == "application/ecmascript"
            || essence == "text/ecmascript"
        {
            return Self::JavaScript;
        }
        if essence == "application/x-www-form-urlencoded" || essence == "multipart/form-data" {
            return Self::Form;
        }
        if essence.starts_with("image/") {
            return Self::Image;
        }
        if essence.starts_with("font/") || essence.starts_with("application/font") {
            return Self::Font;
        }
        if essence.starts_with("text/") {
            return Self::Text;
        }
        if essence.starts_with("audio/")
            || essence.starts_with("video/")
            || essence.starts_with("application/octet-stream")
            || essence.starts_with("application/zip")
            || essence.starts_with("application/gzip")
            || essence.starts_with("application/x-")
            || essence.starts_with("application/pdf")
            || essence.starts_with("application/protobuf")
        {
            return Self::Binary;
        }
        Self::Other
    }

    /// Stable database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Xml => "xml",
            Self::Html => "html",
            Self::Css => "css",
            Self::JavaScript => "js",
            Self::Text => "text",
            Self::Image => "image",
            Self::Font => "font",
            Self::Form => "form",
            Self::Binary => "binary",
            Self::Other => "other",
        }
    }

    /// Parses the stable spelling.
    ///
    /// # Errors
    ///
    /// Returns the offending text when it is not a known media kind.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "xml" => Ok(Self::Xml),
            "html" => Ok(Self::Html),
            "css" => Ok(Self::Css),
            "js" => Ok(Self::JavaScript),
            "text" => Ok(Self::Text),
            "image" => Ok(Self::Image),
            "font" => Ok(Self::Font),
            "form" => Ok(Self::Form),
            "binary" => Ok(Self::Binary),
            "other" => Ok(Self::Other),
            other => Err(other.to_owned()),
        }
    }

    /// Short label for UI badges.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Xml => "xml",
            Self::Html => "html",
            Self::Css => "css",
            Self::JavaScript => "js",
            Self::Text => "text",
            Self::Image => "image",
            Self::Font => "font",
            Self::Form => "form",
            Self::Binary => "binary",
            Self::Other => "-",
        }
    }

    /// `true` when the payload can be shown to a human as text.
    #[must_use]
    pub fn is_textual(self) -> bool {
        matches!(
            self,
            Self::Json
                | Self::Xml
                | Self::Html
                | Self::Css
                | Self::JavaScript
                | Self::Text
                | Self::Form
        )
    }

    /// Every variant, in display order.
    pub const ALL: [Self; 11] = [
        Self::Json,
        Self::Xml,
        Self::Html,
        Self::Css,
        Self::JavaScript,
        Self::Text,
        Self::Image,
        Self::Font,
        Self::Form,
        Self::Binary,
        Self::Other,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_validation_follows_token_grammar() {
        assert_eq!(HttpMethod::parse("get").unwrap().as_str(), "GET");
        assert_eq!(HttpMethod::parse("PROPFIND").unwrap().as_str(), "PROPFIND");
        assert!(HttpMethod::parse("").is_err());
        assert!(HttpMethod::parse("BAD METHOD").is_err());
    }

    #[test]
    fn idempotency_matches_rfc_semantics() {
        assert!(HttpMethod::parse("HEAD").unwrap().is_safe());
        assert!(HttpMethod::parse("PUT").unwrap().is_idempotent());
        assert!(!HttpMethod::parse("POST").unwrap().is_idempotent());
    }

    #[test]
    fn status_codes_classify_by_range() {
        let code = StatusCode::try_new(404).unwrap();
        assert!(code.is_client_error() && code.is_error());
        assert!(StatusCode::try_new(99).is_err());
    }

    #[test]
    fn urls_keep_raw_text_when_unparsable() {
        let url = RequestUrl::new("about:blank");
        assert_eq!(url.as_str(), "about:blank");
        assert_eq!(url.host(), None);
    }

    #[test]
    fn urls_expose_origin_and_path() {
        let url = RequestUrl::new("https://api.example.com:8443/v1/items?q=1");
        assert_eq!(url.host(), Some("api.example.com"));
        assert_eq!(
            url.origin().as_deref(),
            Some("https://api.example.com:8443")
        );
        assert_eq!(url.display_path(), "/v1/items?q=1");
    }

    #[test]
    fn content_types_parse_essence_and_charset() {
        let parsed = ContentType::parse("Application/JSON; charset=UTF-8");
        assert_eq!(parsed.essence(), "application/json");
        assert_eq!(parsed.charset(), Some("utf-8"));
        assert!(parsed.is_json());
        assert_eq!(parsed.kind(), MediaKind::Json);
    }

    #[test]
    fn vendor_json_suffix_is_json() {
        assert!(ContentType::parse("application/problem+json").is_json());
        assert_eq!(
            ContentType::parse("application/vnd.api+json").kind(),
            MediaKind::Json
        );
    }

    #[test]
    fn binary_types_are_not_textual() {
        let kind = ContentType::parse("image/png").kind();
        assert!(!kind.is_textual());
        assert_eq!(kind.label(), "image");
    }
}
