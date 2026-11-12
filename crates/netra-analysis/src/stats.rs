//! Aggregated statistics for a session view.

use std::collections::BTreeMap;

use netra_core::domain::{CaptureSource, ExchangeState, ExchangeSummary, MediaKind};

/// Breakdown of a set of exchanges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExchangeStats {
    /// Total exchanges considered.
    pub total: u64,
    /// Failed plus aborted.
    pub errors: u64,
    /// Still pending.
    pub pending: u64,
    /// Per-host counts, sorted by host.
    pub by_host: Vec<(String, u64)>,
    /// Per-method counts, sorted by method.
    pub by_method: Vec<(String, u64)>,
    /// Per status class counts (`2xx`, `4xx`, ...).
    pub by_status_class: Vec<(String, u64)>,
    /// Per media kind counts.
    pub by_media_kind: Vec<(String, u64)>,
    /// Per source counts.
    pub by_source: Vec<(String, u64)>,
}

impl ExchangeStats {
    /// Aggregates already-filtered summaries.
    #[must_use]
    pub fn from_summaries(summaries: &[ExchangeSummary]) -> Self {
        let mut stats = Self {
            total: summaries.len() as u64,
            ..Self::default()
        };
        let mut hosts: BTreeMap<String, u64> = BTreeMap::new();
        let mut methods: BTreeMap<String, u64> = BTreeMap::new();
        let mut classes: BTreeMap<String, u64> = BTreeMap::new();
        let mut kinds: BTreeMap<String, u64> = BTreeMap::new();
        let mut sources: BTreeMap<String, u64> = BTreeMap::new();
        for summary in summaries {
            match summary.state {
                ExchangeState::Failed | ExchangeState::Aborted => stats.errors += 1,
                ExchangeState::Pending => stats.pending += 1,
                ExchangeState::Completed => {}
            }
            if let Some(host) = summary.host.clone() {
                *hosts.entry(host).or_insert(0) += 1;
            }
            *methods.entry(summary.method.to_string()).or_insert(0) += 1;
            if let Some(status) = summary.status {
                *classes.entry(format!("{}xx", status.class())).or_insert(0) += 1;
            } else {
                *classes.entry("none".to_owned()).or_insert(0) += 1;
            }
            *kinds
                .entry(media_label(summary.media_kind).to_owned())
                .or_insert(0) += 1;
            *sources
                .entry(source_label(summary.source).to_owned())
                .or_insert(0) += 1;
        }
        stats.by_host = hosts.into_iter().collect();
        stats.by_method = methods.into_iter().collect();
        stats.by_status_class = classes.into_iter().collect();
        stats.by_media_kind = kinds.into_iter().collect();
        stats.by_source = sources.into_iter().collect();
        stats
    }
}

fn media_label(kind: MediaKind) -> &'static str {
    kind.label()
}

fn source_label(source: CaptureSource) -> &'static str {
    source.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use netra_core::domain::{
        DurationMs, ExchangeId, HttpMethod, RequestUrl, SessionId, StatusCode, Timestamp,
    };

    fn summary(
        host: &str,
        method: &str,
        status: Option<u16>,
        state: ExchangeState,
    ) -> ExchangeSummary {
        ExchangeSummary {
            id: ExchangeId::new(),
            session_id: SessionId::new(),
            method: HttpMethod::parse(method).unwrap(),
            url: RequestUrl::new(format!("https://{host}/")),
            host: Some(host.to_owned()),
            status: status.map(|code| StatusCode::try_new(code).unwrap()),
            duration: Some(DurationMs::from_millis(1)),
            size_bytes: 1,
            media_kind: MediaKind::Json,
            source: CaptureSource::Proxy,
            state,
            started_at: Timestamp::from_unix_millis(1),
            error: None,
        }
    }

    #[test]
    fn counters_and_breakdowns_are_aggregated() {
        let summaries = vec![
            summary("a.test", "GET", Some(200), ExchangeState::Completed),
            summary("a.test", "POST", Some(500), ExchangeState::Completed),
            summary("b.test", "GET", None, ExchangeState::Failed),
            summary("b.test", "GET", None, ExchangeState::Pending),
        ];
        let stats = ExchangeStats::from_summaries(&summaries);
        assert_eq!(stats.total, 4);
        assert_eq!(stats.errors, 1);
        assert_eq!(stats.pending, 1);
        assert_eq!(
            stats.by_host,
            vec![("a.test".to_owned(), 2), ("b.test".to_owned(), 2)]
        );
        assert_eq!(
            stats.by_method,
            vec![("GET".to_owned(), 3), ("POST".to_owned(), 1)]
        );
        assert_eq!(
            stats.by_status_class,
            vec![
                ("2xx".to_owned(), 1),
                ("5xx".to_owned(), 1),
                ("none".to_owned(), 2)
            ]
        );
        assert_eq!(stats.by_source, vec![("proxy".to_owned(), 4)]);
    }

    #[test]
    fn empty_input_produces_zeroed_stats() {
        let stats = ExchangeStats::from_summaries(&[]);
        assert_eq!(stats.total, 0);
        assert!(stats.by_host.is_empty());
    }
}
