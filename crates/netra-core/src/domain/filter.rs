//! Read-side projections and filters.
//!
//! Filters are pure predicates over [`ExchangeSummary`]. They never mutate
//! stored records: clearing a filter only changes what the UI shows.

use serde::{Deserialize, Serialize};

use super::{
    exchange::{CaptureSource, ExchangeState},
    http::{HttpMethod, MediaKind, RequestUrl, StatusCode},
    ids::{ExchangeId, SessionId},
    time::{DurationMs, Timestamp},
};

/// A set of inclusive status-code ranges (`2xx`, `404`, `500-503`, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusRange {
    ranges: Vec<(u16, u16)>,
}

impl StatusRange {
    /// An empty range set, which matches nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an inclusive range.
    pub fn push(&mut self, from: u16, to: u16) {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        self.ranges.push((from, to));
    }

    /// `true` when no range was configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// `true` when `code` falls inside any configured range.
    #[must_use]
    pub fn matches(&self, code: StatusCode) -> bool {
        if self.ranges.is_empty() {
            return true;
        }
        let value = code.as_u16();
        self.ranges
            .iter()
            .any(|(from, to)| value >= *from && value <= *to)
    }

    /// Parses a comma-separated list of codes, ranges and classes.
    ///
    /// Accepted forms: `200`, `500-503`, `2xx`, `4XX`.
    ///
    /// # Errors
    ///
    /// Returns a description of the first invalid term.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut range = Self::new();
        for term in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if term.len() == 3 && term[1..].eq_ignore_ascii_case("xx") {
                let class: u16 = term[..1]
                    .parse()
                    .map_err(|_| format!("invalid class: {term}"))?;
                if !(1..=5).contains(&class) {
                    return Err(format!("invalid class: {term}"));
                }
                range.push(class * 100, class * 100 + 99);
                continue;
            }
            match term.split_once('-') {
                Some((from, to)) => {
                    let from: u16 = from
                        .trim()
                        .parse()
                        .map_err(|_| format!("invalid code: {term}"))?;
                    let to: u16 = to
                        .trim()
                        .parse()
                        .map_err(|_| format!("invalid code: {term}"))?;
                    StatusCode::try_new(from).map_err(|_| format!("invalid code: {from}"))?;
                    StatusCode::try_new(to).map_err(|_| format!("invalid code: {to}"))?;
                    range.push(from, to);
                }
                None => {
                    let code: u16 = term.parse().map_err(|_| format!("invalid code: {term}"))?;
                    StatusCode::try_new(code).map_err(|_| format!("invalid code: {code}"))?;
                    range.push(code, code);
                }
            }
        }
        Ok(range)
    }

    /// Renders the configured ranges back to CLI/UI text.
    #[must_use]
    pub fn to_text(&self) -> String {
        self.ranges
            .iter()
            .map(|(from, to)| {
                if from == to {
                    from.to_string()
                } else if from % 100 == 0 && *to == from + 99 {
                    format!("{}xx", from / 100)
                } else {
                    format!("{from}-{to}")
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Row shown in the request list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeSummary {
    /// Exchange identifier.
    pub id: ExchangeId,
    /// Owning session.
    pub session_id: SessionId,
    /// HTTP method.
    pub method: HttpMethod,
    /// Full URL as captured.
    pub url: RequestUrl,
    /// Host, when the URL parsed.
    pub host: Option<String>,
    /// Response status, when available.
    pub status: Option<StatusCode>,
    /// Observed duration, when available.
    pub duration: Option<DurationMs>,
    /// Total payload size in bytes.
    pub size_bytes: u64,
    /// Coarse response media type.
    pub media_kind: MediaKind,
    /// Capture source.
    pub source: CaptureSource,
    /// Lifecycle state.
    pub state: ExchangeState,
    /// When the request was observed.
    pub started_at: Timestamp,
    /// Failure description, when failed or aborted.
    pub error: Option<String>,
}

/// A conjunctive set of list filters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeFilter {
    /// Case-insensitive substring over URL and error text.
    pub text: Option<String>,
    /// Case-insensitive host match (`api.example.com`).
    pub host: Option<String>,
    /// Any of these methods (empty means every method).
    pub methods: Vec<HttpMethod>,
    /// Status-code ranges.
    pub status: StatusRange,
    /// Any of these media kinds.
    pub media_kinds: Vec<MediaKind>,
    /// Any of these sources.
    pub sources: Vec<CaptureSource>,
    /// Any of these states.
    pub states: Vec<ExchangeState>,
    /// Keep only failed and aborted exchanges.
    pub only_failed: bool,
}

impl ExchangeFilter {
    /// `true` when the filter cannot exclude anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.as_ref().is_none_or(|text| text.is_empty())
            && self.host.is_none()
            && self.methods.is_empty()
            && self.status.is_empty()
            && self.media_kinds.is_empty()
            && self.sources.is_empty()
            && self.states.is_empty()
            && !self.only_failed
    }

    /// `true` when `summary` satisfies every configured criterion.
    #[must_use]
    pub fn matches(&self, summary: &ExchangeSummary) -> bool {
        if let Some(text) = self
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            let needle = text.to_lowercase();
            let haystack_url = summary.url.as_str().to_lowercase();
            let haystack_error = summary.error.as_deref().unwrap_or_default().to_lowercase();
            if !haystack_url.contains(&needle) && !haystack_error.contains(&needle) {
                return false;
            }
        }
        if let Some(host) = self
            .host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            && !summary
                .host
                .as_deref()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(host))
        {
            return false;
        }
        if !self.methods.is_empty() && !self.methods.contains(&summary.method) {
            return false;
        }
        if let Some(status) = summary.status {
            if !self.status.matches(status) {
                return false;
            }
        } else if !self.status.is_empty() {
            return false;
        }
        if !self.media_kinds.is_empty() && !self.media_kinds.contains(&summary.media_kind) {
            return false;
        }
        if !self.sources.is_empty() && !self.sources.contains(&summary.source) {
            return false;
        }
        if !self.states.is_empty() && !self.states.contains(&summary.state) {
            return false;
        }
        if self.only_failed
            && !matches!(
                summary.state,
                ExchangeState::Failed | ExchangeState::Aborted
            )
        {
            return false;
        }
        true
    }
}

/// Pagination and ordering for list queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeQuery {
    /// Session to read.
    pub session_id: SessionId,
    /// Predicate applied by the storage adapter.
    pub filter: ExchangeFilter,
    /// Maximum number of rows to return.
    pub limit: usize,
    /// Rows to skip.
    pub offset: usize,
    /// Ordering of the result window.
    pub order: ExchangeOrder,
}

impl ExchangeQuery {
    /// A query returning the newest `limit` exchanges of a session.
    #[must_use]
    pub fn newest(session_id: SessionId, limit: usize) -> Self {
        Self {
            session_id,
            filter: ExchangeFilter::default(),
            limit,
            offset: 0,
            order: ExchangeOrder::StartedDesc,
        }
    }
}

/// Result ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeOrder {
    /// Oldest first, as a waterfall reads.
    StartedAsc,
    /// Newest first, as a live log reads.
    #[default]
    StartedDesc,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        method: &str,
        url: &str,
        status: Option<u16>,
        state: ExchangeState,
    ) -> ExchangeSummary {
        let url = RequestUrl::new(url);
        ExchangeSummary {
            id: ExchangeId::new(),
            session_id: SessionId::new(),
            method: HttpMethod::parse(method).unwrap(),
            host: url.host().map(str::to_owned),
            url,
            status: status.map(|code| StatusCode::try_new(code).unwrap()),
            duration: Some(DurationMs::from_millis(10)),
            size_bytes: 10,
            media_kind: MediaKind::Json,
            source: CaptureSource::Cdp,
            state,
            started_at: Timestamp::from_unix_millis(1),
            error: None,
        }
    }

    #[test]
    fn empty_filter_matches_everything() {
        let filter = ExchangeFilter::default();
        assert!(filter.is_empty());
        assert!(filter.matches(&summary(
            "GET",
            "https://a.test/",
            Some(200),
            ExchangeState::Completed
        )));
    }

    #[test]
    fn text_filter_is_case_insensitive_over_url() {
        let filter = ExchangeFilter {
            text: Some("API.EXAMPLE".into()),
            ..ExchangeFilter::default()
        };
        assert!(filter.matches(&summary(
            "GET",
            "https://api.example.com/v1",
            Some(200),
            ExchangeState::Completed
        )));
        assert!(!filter.matches(&summary(
            "GET",
            "https://other.test/",
            Some(200),
            ExchangeState::Completed
        )));
    }

    #[test]
    fn status_ranges_parse_classes_ranges_and_codes() {
        let range = StatusRange::parse("2xx,404,500-503").unwrap();
        assert!(range.matches(StatusCode::try_new(204).unwrap()));
        assert!(range.matches(StatusCode::try_new(404).unwrap()));
        assert!(range.matches(StatusCode::try_new(502).unwrap()));
        assert!(!range.matches(StatusCode::try_new(304).unwrap()));
        assert_eq!(range.to_text(), "2xx,404,500-503");
    }

    #[test]
    fn invalid_status_terms_are_rejected() {
        assert!(StatusRange::parse("2xx,abc").is_err());
        assert!(StatusRange::parse("700").is_err());
        assert!(StatusRange::parse("9xx").is_err());
    }

    #[test]
    fn pending_rows_do_not_satisfy_status_filters() {
        let filter = ExchangeFilter {
            status: StatusRange::parse("2xx").unwrap(),
            ..ExchangeFilter::default()
        };
        assert!(!filter.matches(&summary(
            "GET",
            "https://a.test/",
            None,
            ExchangeState::Pending
        )));
    }

    #[test]
    fn method_and_failure_filters_combine() {
        let filter = ExchangeFilter {
            methods: vec![HttpMethod::parse("POST").unwrap()],
            only_failed: true,
            ..ExchangeFilter::default()
        };
        let mut failed = summary("POST", "https://a.test/x", None, ExchangeState::Failed);
        failed.error = Some("reset".into());
        assert!(filter.matches(&failed));
        assert!(!filter.matches(&summary(
            "GET",
            "https://a.test/x",
            None,
            ExchangeState::Failed
        )));
        assert!(!filter.matches(&summary(
            "POST",
            "https://a.test/x",
            Some(500),
            ExchangeState::Completed
        )));
    }

    #[test]
    fn host_filter_matches_exactly() {
        let filter = ExchangeFilter {
            host: Some("api.test".into()),
            ..ExchangeFilter::default()
        };
        assert!(filter.matches(&summary(
            "GET",
            "https://api.test/a",
            Some(200),
            ExchangeState::Completed
        )));
        assert!(!filter.matches(&summary(
            "GET",
            "https://api.test.evil/a",
            Some(200),
            ExchangeState::Completed
        )));
    }
}
