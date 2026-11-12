//! Request and response payloads.
//!
//! A body is never just "some bytes": the inspector must tell apart a body that
//! was absent, one that was omitted (too large, unreadable), one withheld by the
//! redaction policy, and one stored but truncated.

use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

use super::http::ContentType;

/// Why a payload was not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmitReason {
    /// The payload exceeded the configured capture limit.
    TooLarge,
    /// The capture source does not expose this body.
    NotProvidedBySource,
    /// Retrieval was attempted and failed.
    RetrievalFailed,
    /// The payload could not be decoded (bad compression, broken base64, ...).
    Undecodable,
}

impl OmitReason {
    /// Human-readable label used in the inspector.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TooLarge => "too large",
            Self::NotProvidedBySource => "not provided by source",
            Self::RetrievalFailed => "retrieval failed",
            Self::Undecodable => "undecodable",
        }
    }
}

/// The payload of a request or response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    /// Declared media type, when a `Content-Type` header was present.
    pub content_type: Option<ContentType>,
    /// Declared `Content-Encoding` (for example `gzip`), when present.
    pub encoding: Option<String>,
    /// What happened to the payload itself.
    pub data: BodyData,
    /// Size in bytes as reported by the source or, failing that, as stored.
    pub size_bytes: u64,
    /// `true` when only a prefix of the payload is stored.
    pub truncated: bool,
}

impl Body {
    /// A body that does not exist (a `GET` without payload).
    #[must_use]
    pub fn absent() -> Self {
        Self {
            content_type: None,
            encoding: None,
            data: BodyData::Absent,
            size_bytes: 0,
            truncated: false,
        }
    }

    /// A body that was captured in full.
    #[must_use]
    pub fn captured(content_type: Option<ContentType>, bytes: Vec<u8>) -> Self {
        let size_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        Self {
            content_type,
            encoding: None,
            data: BodyData::Captured { bytes },
            size_bytes,
            truncated: false,
        }
    }

    /// A body known to exist but deliberately not stored.
    #[must_use]
    pub fn omitted(content_type: Option<ContentType>, reason: OmitReason, size_bytes: u64) -> Self {
        Self {
            content_type,
            encoding: None,
            data: BodyData::Omitted { reason },
            size_bytes,
            truncated: false,
        }
    }

    /// A body withheld by the redaction policy.
    #[must_use]
    pub fn redacted(content_type: Option<ContentType>, size_bytes: u64) -> Self {
        Self {
            content_type,
            encoding: None,
            data: BodyData::Redacted,
            size_bytes,
            truncated: false,
        }
    }

    /// Declares the transfer encoding.
    #[must_use]
    pub fn with_encoding(mut self, encoding: Option<String>) -> Self {
        self.encoding = encoding;
        self
    }

    /// Declares the reported size, which may exceed the stored prefix.
    #[must_use]
    pub fn with_size(mut self, size_bytes: u64) -> Self {
        self.size_bytes = size_bytes;
        self
    }

    /// `true` when no payload bytes exist at all.
    #[must_use]
    pub fn is_absent(&self) -> bool {
        matches!(self.data, BodyData::Absent)
    }

    /// Returns stored bytes, if any.
    #[must_use]
    pub fn bytes(&self) -> Option<&[u8]> {
        match &self.data {
            BodyData::Captured { bytes } => Some(bytes),
            _ => None,
        }
    }

    /// Returns the stored payload decoded as UTF-8 (lossy) when available.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        self.bytes()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }

    /// Enforces a storage limit on the payload, keeping the reported size.
    ///
    /// Returns `true` when the payload was truncated by this call.
    pub fn apply_limit(&mut self, max_bytes: usize) -> bool {
        let Some(bytes) = self.bytes() else {
            return false;
        };
        if bytes.len() <= max_bytes {
            return false;
        }
        let prefix = bytes[..max_bytes].to_vec();
        self.size_bytes = self
            .size_bytes
            .max(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.data = BodyData::Captured { bytes: prefix };
        self.truncated = true;
        true
    }

    /// Replaces any stored payload with a redaction marker.
    pub fn redact(&mut self) {
        if matches!(self.data, BodyData::Captured { .. }) {
            self.data = BodyData::Redacted;
            self.truncated = false;
        }
    }

    /// Short description for the inspector header.
    #[must_use]
    pub fn describe(&self) -> String {
        let base = match &self.data {
            BodyData::Absent => "absent".to_owned(),
            BodyData::Captured { .. } if self.truncated => format!(
                "truncated at {} of {}",
                human_bytes(u64::try_from(self.bytes().map_or(0, <[u8]>::len)).unwrap_or(0)),
                human_bytes(self.size_bytes)
            ),
            BodyData::Captured { .. } => human_bytes(self.size_bytes),
            BodyData::Omitted { reason } => {
                format!(
                    "omitted ({}); {}",
                    reason.label(),
                    human_bytes(self.size_bytes)
                )
            }
            BodyData::Redacted => "redacted".to_owned(),
        };
        match (&self.content_type, &self.encoding) {
            (Some(content_type), Some(encoding)) => format!("{base} · {content_type} · {encoding}"),
            (Some(content_type), None) => format!("{base} · {content_type}"),
            (None, Some(encoding)) => format!("{base} · {encoding}"),
            (None, None) => base,
        }
    }
}

/// Payload storage outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BodyData {
    /// No payload exists.
    Absent,
    /// The payload is stored (possibly truncated; see [`Body::truncated`]).
    Captured {
        /// Raw bytes, base64-encoded when serialized as JSON.
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
    },
    /// The payload exists but was not stored.
    Omitted {
        /// Why it was not stored.
        reason: OmitReason,
    },
    /// The payload was withheld by policy.
    Redacted,
}

mod base64_bytes {
    use super::BASE64;
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&BASE64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        BASE64.decode(encoded).map_err(D::Error::custom)
    }
}

/// Formats a byte count with binary units.
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(content_type: &str) -> Option<ContentType> {
        Some(ContentType::parse(content_type))
    }

    #[test]
    fn absent_and_captured_are_distinguishable() {
        assert!(Body::absent().is_absent());
        let captured = Body::captured(json("application/json"), b"{\"a\":1}".to_vec());
        assert_eq!(captured.size_bytes, 7);
        assert!(!captured.truncated);
        assert_eq!(captured.text().as_deref(), Some("{\"a\":1}"));
    }

    #[test]
    fn limit_truncates_and_keeps_declared_size() {
        let mut body = Body::captured(json("text/plain"), b"0123456789".to_vec()).with_size(4096);
        assert!(body.apply_limit(4));
        assert!(body.truncated);
        assert_eq!(body.bytes().unwrap(), b"0123");
        assert_eq!(body.size_bytes, 4096);
        assert!(body.describe().contains("truncated"));
    }

    #[test]
    fn limit_is_noop_for_small_payloads() {
        let mut body = Body::captured(None, b"abc".to_vec());
        assert!(!body.apply_limit(10));
        assert!(!body.truncated);
    }

    #[test]
    fn redaction_drops_bytes_but_keeps_size() {
        let mut body = Body::captured(json("application/json"), b"{\"token\":1}".to_vec());
        body.redact();
        assert!(body.bytes().is_none());
        assert_eq!(body.size_bytes, 11);
        assert_eq!(body.describe(), "redacted · application/json");
    }

    #[test]
    fn omitted_reports_reason() {
        let body = Body::omitted(json("audio/mpeg"), OmitReason::TooLarge, 5_000_000);
        assert!(body.describe().contains("too large"));
        assert!(body.bytes().is_none());
    }

    #[test]
    fn captured_bytes_round_trip_as_base64() {
        let body = Body::captured(None, vec![0, 1, 2, 250]);
        let encoded = serde_json::to_string(&body).unwrap();
        assert!(encoded.contains("AAEC+g=="));
        let decoded: Body = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, body);
    }

    #[test]
    fn byte_counts_use_binary_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1_500), "1.5 kB");
        assert_eq!(human_bytes(2_500_000), "2.5 MB");
    }
}
