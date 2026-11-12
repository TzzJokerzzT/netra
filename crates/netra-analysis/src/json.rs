//! JSON inspection.
//!
//! Pretty-printing reuses the captured text when it already parses; nothing is
//! evaluated and no scripts are ever executed.

/// Result of pretty-printing a JSON document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonView {
    /// Pretty-printed text, or the original text when it does not parse.
    pub text: String,
    /// `true` when the payload is valid JSON.
    pub valid: bool,
    /// Parser message when the payload is invalid.
    pub error: Option<String>,
}

/// Pretty-prints a JSON payload, tolerating invalid documents.
#[must_use]
pub fn pretty(raw: &str) -> JsonView {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(text) => JsonView {
                text,
                valid: true,
                error: None,
            },
            Err(error) => JsonView {
                text: raw.to_owned(),
                valid: false,
                error: Some(error.to_string()),
            },
        },
        Err(error) => JsonView {
            text: raw.to_owned(),
            valid: false,
            error: Some(error.to_string()),
        },
    }
}

/// Cheap structural test used before trusting a `Content-Type`.
#[must_use]
pub fn looks_like_json(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    (trimmed.starts_with('{') && trimmed.trim_end().ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.trim_end().ends_with(']'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_documents_are_pretty_printed() {
        let view = pretty(r#"{"a":1,"b":[2,3]}"#);
        assert!(view.valid);
        assert!(view.text.contains("\"a\": 1"));
        assert!(view.text.lines().count() > 1);
    }

    #[test]
    fn key_order_is_preserved() {
        let view = pretty(r#"{"z":1,"a":2}"#);
        assert!(view.text.find("\"z\"").unwrap() < view.text.find("\"a\"").unwrap());
    }

    #[test]
    fn invalid_documents_keep_their_text_and_error() {
        let view = pretty("{\"a\": ");
        assert!(!view.valid);
        assert_eq!(view.text, "{\"a\": ");
        assert!(view.error.is_some());
    }

    #[test]
    fn structural_detection_is_lenient() {
        assert!(looks_like_json("  [1,2]  "));
        assert!(looks_like_json("{\"a\":1}"));
        assert!(!looks_like_json("<html></html>"));
    }
}
