//! Time value objects.
//!
//! Wall-clock instants and elapsed durations are deliberately distinct types:
//! a [`Timestamp`] answers *when*, a [`DurationMs`] answers *how long*.

use std::{
    fmt,
    ops::Sub,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// An absolute UTC instant with millisecond precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    /// Captures the current UTC instant truncated to milliseconds.
    #[must_use]
    pub fn now() -> Self {
        Self::from_datetime(Utc::now())
    }

    /// Builds a timestamp from any UTC-aware datetime, truncating to milliseconds.
    #[must_use]
    pub fn from_datetime(value: DateTime<Utc>) -> Self {
        Self(
            DateTime::from_timestamp_millis(value.timestamp_millis())
                .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
        )
    }

    /// Builds a timestamp from milliseconds since the Unix epoch.
    #[must_use]
    pub fn from_unix_millis(millis: i64) -> Self {
        Self(DateTime::from_timestamp_millis(millis).unwrap_or(DateTime::<Utc>::UNIX_EPOCH))
    }

    /// Returns milliseconds since the Unix epoch.
    #[must_use]
    pub fn unix_millis(self) -> i64 {
        self.0.timestamp_millis()
    }

    /// Returns the date-time as understood by `chrono`.
    #[must_use]
    pub const fn as_datetime(self) -> DateTime<Utc> {
        self.0
    }

    /// Formats the instant as RFC 3339 with millisecond precision.
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    /// Parses an RFC 3339 timestamp.
    ///
    /// # Errors
    ///
    /// Returns the underlying `chrono` parse error for malformed input.
    pub fn parse(text: &str) -> Result<Self, chrono::ParseError> {
        DateTime::parse_from_rfc3339(text).map(|v| Self::from_datetime(v.with_timezone(&Utc)))
    }

    /// Builds a timestamp from a [`SystemTime`].
    #[must_use]
    pub fn from_system_time(value: SystemTime) -> Self {
        let millis = value
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        Self::from_unix_millis(millis)
    }

    /// Returns the elapsed time from `self` to `later`, never negative.
    #[must_use]
    pub fn elapsed_until(self, later: Self) -> DurationMs {
        let delta = later.unix_millis().saturating_sub(self.unix_millis());
        DurationMs::from_millis(u64::try_from(delta).unwrap_or(0))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl Sub for Timestamp {
    type Output = DurationMs;

    fn sub(self, rhs: Self) -> Self::Output {
        rhs.elapsed_until(self)
    }
}

/// A duration expressed in whole milliseconds.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct DurationMs(u64);

impl DurationMs {
    /// A zero-length duration.
    pub const ZERO: Self = Self(0);

    /// Builds a duration from whole milliseconds.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis)
    }

    /// Builds a duration from a standard [`Duration`], truncating sub-millisecond parts.
    #[must_use]
    pub fn from_std(value: Duration) -> Self {
        Self(u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
    }

    /// Returns the duration in whole milliseconds.
    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// Returns the duration in fractional seconds.
    #[must_use]
    pub fn as_secs_f64(self) -> f64 {
        (self.0 as f64) / 1000.0
    }

    /// Converts to a standard [`Duration`].
    #[must_use]
    pub const fn as_std(self) -> Duration {
        Duration::from_millis(self.0)
    }

    /// Adds two durations, saturating on overflow.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl fmt::Display for DurationMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0..=999 => write!(f, "{} ms", self.0),
            1000..=59_999 => write!(f, "{:.1} s", self.as_secs_f64()),
            _ => {
                let total_secs = self.0 / 1000;
                write!(f, "{}m{:02}s", total_secs / 60, total_secs % 60)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_round_trips_through_rfc3339() {
        let now = Timestamp::now();
        let parsed = Timestamp::parse(&now.to_rfc3339()).unwrap();
        assert_eq!(now, parsed);
    }

    #[test]
    fn elapsed_is_not_negative_for_reordered_instants() {
        let early = Timestamp::from_unix_millis(1_000);
        let late = Timestamp::from_unix_millis(1_750);
        assert_eq!(early.elapsed_until(late), DurationMs::from_millis(750));
        assert_eq!(late.elapsed_until(early), DurationMs::ZERO);
    }

    #[test]
    fn durations_render_human_readable_units() {
        assert_eq!(DurationMs::from_millis(12).to_string(), "12 ms");
        assert_eq!(DurationMs::from_millis(1_500).to_string(), "1.5 s");
        assert_eq!(DurationMs::from_millis(65_000).to_string(), "1m05s");
    }
}
