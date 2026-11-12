//! Choosing how a captured payload is displayed.
//!
//! Binary data is never printed as text: it is shown as a bounded hex dump with
//! its size, so a terminal cannot be flooded or corrupted.

use netra_core::domain::{Body, BodyData, human_bytes};

/// Rendered payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyView {
    /// Nothing to show, with the reason (`absent`, `redacted`, `omitted`).
    Empty {
        /// Human-readable explanation.
        description: String,
    },
    /// Plain text (HTML, XML, CSS, JS, form data, ...).
    Text {
        /// Rendered text.
        text: String,
        /// `true` when only a prefix is available.
        truncated: bool,
    },
    /// JSON, pretty-printed when it parses.
    Json {
        /// Rendered text.
        text: String,
        /// `true` when the document parses.
        valid: bool,
        /// Parser message when it does not parse.
        error: Option<String>,
        /// `true` when only a prefix is available.
        truncated: bool,
    },
    /// Binary payload shown as a hex dump.
    Hex {
        /// Rendered hex rows (16 bytes per row).
        rows: Vec<String>,
        /// Bytes rendered.
        shown_bytes: usize,
        /// Declared total size.
        total_bytes: u64,
    },
}

/// Bytes rendered in one hex dump.
pub const HEX_PREVIEW_BYTES: usize = 256;

impl BodyView {
    /// Builds the view of a payload.
    #[must_use]
    pub fn from_body(body: &Body, max_text_bytes: usize) -> Self {
        if let BodyData::Omitted { reason } = &body.data {
            return Self::Empty {
                description: format!(
                    "omitted: {} ({})",
                    reason.label(),
                    human_bytes(body.size_bytes)
                ),
            };
        }
        if matches!(body.data, BodyData::Redacted) {
            return Self::Empty {
                description: format!("redacted ({} withheld)", human_bytes(body.size_bytes)),
            };
        }
        let Some(bytes) = body.bytes() else {
            return Self::Empty {
                description: "no payload".to_owned(),
            };
        };
        let kind = body
            .content_type
            .as_ref()
            .map_or(netra_core::domain::MediaKind::Other, |content_type| {
                content_type.kind()
            });
        if kind.is_textual() || looks_textual(bytes) {
            let mut text = String::from_utf8_lossy(bytes).into_owned();
            let mut truncated = body.truncated;
            if text.len() > max_text_bytes {
                let cut = floor_char_boundary(&text, max_text_bytes);
                text.truncate(cut);
                truncated = true;
            }
            if kind == netra_core::domain::MediaKind::Json || crate::json::looks_like_json(&text) {
                let view = crate::json::pretty(&text);
                return Self::Json {
                    text: view.text,
                    valid: view.valid,
                    error: view.error,
                    truncated,
                };
            }
            return Self::Text { text, truncated };
        }
        let shown = &bytes[..bytes.len().min(HEX_PREVIEW_BYTES)];
        Self::Hex {
            rows: hex_rows(shown),
            shown_bytes: shown.len(),
            total_bytes: body.size_bytes,
        }
    }

    /// `true` when there is nothing to display.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty { .. })
    }
}

/// `true` when the bytes are valid UTF-8 and contain no control characters
/// beyond tab, newline and carriage return.
fn looks_textual(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(text) => !text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\n' | '\r')),
        Err(_) => false,
    }
}

/// Rounds down to the nearest `char` boundary at or below `limit`.
fn floor_char_boundary(text: &str, limit: usize) -> usize {
    let mut index = limit.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Formats bytes as classic 16-column hex rows.
fn hex_rows(bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(16)
        .enumerate()
        .map(|(row, chunk)| {
            let offset = row * 16;
            let hex: Vec<String> = chunk.iter().map(|byte| format!("{byte:02x}")).collect();
            let ascii: String = chunk
                .iter()
                .map(|byte| {
                    if byte.is_ascii_graphic() || *byte == b' ' {
                        char::from(*byte)
                    } else {
                        '.'
                    }
                })
                .collect();
            format!("{offset:08x}  {:<47}  {ascii}", hex.join(" "))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use netra_core::domain::{ContentType, OmitReason};

    #[test]
    fn json_payloads_are_pretty_printed() {
        let body = Body::captured(
            Some(ContentType::parse("application/json")),
            br#"{"a":1}"#.to_vec(),
        );
        match BodyView::from_body(&body, 1024) {
            BodyView::Json { text, valid, .. } => {
                assert!(valid);
                assert!(text.contains("\"a\": 1"));
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn html_is_shown_as_text_not_executed() {
        let body = Body::captured(
            Some(ContentType::parse("text/html")),
            b"<script>alert(1)</script>".to_vec(),
        );
        match BodyView::from_body(&body, 1024) {
            BodyView::Text { text, .. } => assert_eq!(text, "<script>alert(1)</script>"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn binary_payloads_get_a_bounded_hex_dump() {
        let mut bytes = vec![0u8; 100];
        bytes[0] = 0x89;
        bytes[1] = b'P';
        let body = Body::captured(Some(ContentType::parse("image/png")), bytes);
        match BodyView::from_body(&body, 1024) {
            BodyView::Hex {
                rows,
                shown_bytes,
                total_bytes,
            } => {
                assert_eq!(shown_bytes, 100);
                assert_eq!(rows.len(), 7);
                assert!(rows[0].starts_with("00000000  89 50"));
                assert_eq!(total_bytes, 100);
            }
            other => panic!("expected Hex, got {other:?}"),
        }
    }

    #[test]
    fn hex_dump_is_capped() {
        let body = Body::captured(
            Some(ContentType::parse("application/octet-stream")),
            vec![0; 4096],
        );
        match BodyView::from_body(&body, 1024) {
            BodyView::Hex {
                shown_bytes,
                total_bytes,
                ..
            } => {
                assert_eq!(shown_bytes, HEX_PREVIEW_BYTES);
                assert_eq!(total_bytes, 4096);
            }
            other => panic!("expected Hex, got {other:?}"),
        }
    }

    #[test]
    fn redacted_and_omitted_payloads_report_why() {
        let redacted = BodyView::from_body(&Body::redacted(None, 100), 1024);
        assert!(matches!(redacted, BodyView::Empty { .. }));
        let omitted =
            BodyView::from_body(&Body::omitted(None, OmitReason::TooLarge, 5_000_000), 1024);
        match omitted {
            BodyView::Empty { description } => assert!(description.contains("too large")),
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    #[test]
    fn long_text_is_truncated_on_a_char_boundary() {
        let body = Body::captured(
            Some(ContentType::parse("text/plain")),
            "áéíóú".repeat(100).into_bytes(),
        );
        match BodyView::from_body(&body, 7) {
            BodyView::Text { text, truncated } => {
                assert!(truncated);
                assert!(text.len() <= 7);
                assert!(std::str::from_utf8(text.as_bytes()).is_ok());
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn control_bytes_without_a_content_type_are_not_printed_as_text() {
        let body = Body::captured(None, vec![0x00, 0x01, 0x02, 0x03]);
        assert!(matches!(
            BodyView::from_body(&body, 1024),
            BodyView::Hex { .. }
        ));
    }
}
