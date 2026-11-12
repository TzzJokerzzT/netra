//! Waterfall data for the Timing tab.
//!
//! Only real measurements are represented: when a source reports no phases the
//! result is empty rather than an invented breakdown.

use netra_core::domain::{DurationMs, NetworkExchange};

/// One measured phase, positioned as a ratio of the total.
#[derive(Debug, Clone, PartialEq)]
pub struct WaterfallSegment {
    /// Phase name (`dns`, `connect`, `tls`, `ttfb`, `download`, ...).
    pub label: &'static str,
    /// Measured duration.
    pub duration: DurationMs,
    /// Offset from the start of the exchange, as a fraction of the total.
    pub start_ratio: f64,
    /// Width as a fraction of the total.
    pub width_ratio: f64,
}

/// Waterfall view of one exchange.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Waterfall {
    /// Measured phases in display order.
    pub segments: Vec<WaterfallSegment>,
    /// Total duration used for the ratios.
    pub total: DurationMs,
}

impl Waterfall {
    /// Builds the waterfall for an exchange, or an empty one when the source
    /// reported no phase data.
    #[must_use]
    pub fn from_exchange(exchange: &NetworkExchange) -> Self {
        let Some(timing) = exchange.timing else {
            return Self::default();
        };
        let total = timing
            .total
            .or_else(|| exchange.duration())
            .unwrap_or_else(|| timing.sum_phases());
        if total.as_millis() == 0 {
            return Self::default();
        }
        let mut offset = 0u64;
        let mut segments = Vec::new();
        for (label, duration) in timing.phases() {
            let width = duration.as_millis();
            segments.push(WaterfallSegment {
                label,
                duration,
                start_ratio: (offset as f64) / (total.as_millis() as f64),
                width_ratio: (width as f64) / (total.as_millis() as f64),
            });
            offset = offset.saturating_add(width);
        }
        Self { segments, total }
    }

    /// `true` when no phase was measured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netra_core::domain::{
        CaptureSource, HttpMethod, NetworkExchange, NetworkRequest, RequestUrl, SessionId,
        Timestamp, TimingMetrics,
    };

    fn exchange(timing: Option<TimingMetrics>) -> NetworkExchange {
        let request = NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("https://example.com/"),
            Timestamp::from_unix_millis(0),
        );
        let mut exchange =
            NetworkExchange::pending(SessionId::new(), CaptureSource::Cdp, request, None);
        exchange.timing = timing;
        exchange
    }

    #[test]
    fn no_timings_produce_no_segments() {
        assert!(Waterfall::from_exchange(&exchange(None)).is_empty());
        assert!(Waterfall::from_exchange(&exchange(Some(TimingMetrics::default()))).is_empty());
    }

    #[test]
    fn segments_are_positioned_by_measured_durations() {
        let timing = TimingMetrics {
            dns: Some(DurationMs::from_millis(10)),
            connect: Some(DurationMs::from_millis(30)),
            ttfb: Some(DurationMs::from_millis(60)),
            total: Some(DurationMs::from_millis(100)),
            ..TimingMetrics::default()
        };
        let waterfall = Waterfall::from_exchange(&exchange(Some(timing)));
        assert_eq!(waterfall.segments.len(), 3);
        assert_eq!(waterfall.segments[0].start_ratio, 0.0);
        assert!((waterfall.segments[1].start_ratio - 0.1).abs() < 1e-9);
        assert!((waterfall.segments[2].width_ratio - 0.6).abs() < 1e-9);
        assert_eq!(waterfall.total, DurationMs::from_millis(100));
    }
}
