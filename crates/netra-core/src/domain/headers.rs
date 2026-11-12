//! Ordered, duplicate-preserving HTTP header collections.
//!
//! Captured headers are evidence: repeated names must survive (a response can
//! carry several `Set-Cookie` lines) and the observed order must not be lost.
//! Lookups are case-insensitive per RFC 9110 without rewriting stored text.

use serde::{Deserialize, Serialize};

/// One header line exactly as observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderEntry {
    /// Header field name as captured.
    pub name: String,
    /// Header field value as captured.
    pub value: String,
}

impl HeaderEntry {
    /// Builds a header entry from any string-like pair.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Case-insensitive name comparison.
    #[must_use]
    pub fn name_is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }
}

/// An ordered list of header lines.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHeaders {
    #[serde(default)]
    entries: Vec<HeaderEntry>,
}

impl HttpHeaders {
    /// An empty header collection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a collection from pairs, preserving order and duplicates.
    pub fn from_pairs<I, N, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: Into<String>,
    {
        Self {
            entries: pairs
                .into_iter()
                .map(|(name, value)| HeaderEntry::new(name, value))
                .collect(),
        }
    }

    /// Appends a header line.
    pub fn push(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.entries.push(HeaderEntry::new(name, value));
    }

    /// Iterates over header lines in captured order.
    pub fn iter(&self) -> impl Iterator<Item = &HeaderEntry> {
        self.entries.iter()
    }

    /// Iterates mutably over header lines in captured order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut HeaderEntry> {
        self.entries.iter_mut()
    }

    /// Number of header lines.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when no header lines were captured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the first value for `name`, case-insensitively.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.name_is(name))
            .map(|entry| entry.value.as_str())
    }

    /// Returns every value for `name`, in captured order.
    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.entries
            .iter()
            .filter(move |entry| entry.name_is(name))
            .map(|entry| entry.value.as_str())
    }

    /// `true` when the header is present, case-insensitively.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|entry| entry.name_is(name))
    }

    /// Replaces the value of every matching line, preserving name spelling.
    pub fn set_value(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        for entry in &mut self.entries {
            if entry.name_is(name) {
                entry.value.clone_from(&value);
            }
        }
    }

    /// Removes every line whose name matches, case-insensitively.
    pub fn remove(&mut self, name: &str) {
        self.entries.retain(|entry| !entry.name_is(name));
    }

    /// Distinct header names in first-seen order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for entry in &self.entries {
            if !seen
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&entry.name))
            {
                seen.push(&entry.name);
            }
        }
        seen
    }
}

impl FromIterator<HeaderEntry> for HttpHeaders {
    fn from_iter<T: IntoIterator<Item = HeaderEntry>>(iter: T) -> Self {
        Self {
            entries: iter.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> HttpHeaders {
        HttpHeaders::from_pairs([
            ("Accept", "*/*"),
            ("Set-Cookie", "a=1"),
            ("set-cookie", "b=2"),
            ("X-Trace", "abc"),
        ])
    }

    #[test]
    fn preserves_duplicates_and_order() {
        let headers = sample();
        assert_eq!(headers.len(), 4);
        assert_eq!(
            headers.get_all("set-cookie").collect::<Vec<_>>(),
            vec!["a=1", "b=2"]
        );
        assert_eq!(headers.iter().next().unwrap().name, "Accept");
    }

    #[test]
    fn lookup_is_case_insensitive_without_rewriting() {
        let headers = sample();
        assert_eq!(headers.get("ACCEPT"), Some("*/*"));
        assert_eq!(headers.get("set-Cookie"), Some("a=1"));
        assert_eq!(headers.names(), vec!["Accept", "Set-Cookie", "X-Trace"]);
    }

    #[test]
    fn remove_and_set_apply_to_all_matches() {
        let mut headers = sample();
        headers.set_value("SET-COOKIE", "redacted");
        assert_eq!(headers.get_all("set-cookie").count(), 2);
        assert!(
            headers
                .get_all("set-cookie")
                .all(|value| value == "redacted")
        );
        headers.remove("Set-Cookie");
        assert!(!headers.contains("set-cookie"));
        assert_eq!(headers.len(), 2);
    }
}
