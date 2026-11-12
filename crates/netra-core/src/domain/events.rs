//! Normalized capture events.
//!
//! Providers translate their native payloads (CDP `Network.*`, proxy
//! request/response) into these events. The domain never mentions CDP names.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    body::Body,
    exchange::{CaptureSource, NetworkRequest, NetworkResponse, TimingMetrics},
};

/// Direction of a payload update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyDirection {
    /// The request payload.
    Request,
    /// The response payload.
    Response,
}

impl BodyDirection {
    /// Stable database and CLI spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
        }
    }

    /// Parses the stable spelling.
    ///
    /// # Errors
    ///
    /// Returns the offending text when it is neither `request` nor `response`.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "request" => Ok(Self::Request),
            "response" => Ok(Self::Response),
            other => Err(other.to_owned()),
        }
    }
}

/// Provider-issued correlation key for one exchange.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceKey {
    /// Which provider produced the key.
    pub source: CaptureSource,
    /// Provider-specific request identifier (CDP `requestId`, proxy sequence).
    pub request_id: String,
}

impl SourceKey {
    /// Builds a correlation key.
    pub fn new(source: CaptureSource, request_id: impl Into<String>) -> Self {
        Self {
            source,
            request_id: request_id.into(),
        }
    }
}

impl fmt::Display for SourceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.source.as_str(), self.request_id)
    }
}

/// An event emitted by a capture provider.
///
/// Variants carry complete request/response descriptors by value: capture
/// channels are bounded and the ownership transfer avoids an allocation per
/// event on the hot path.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureEvent {
    /// A request is known; the exchange must appear immediately, pending.
    RequestStarted {
        /// Correlation key.
        key: SourceKey,
        /// Request data observed so far.
        request: NetworkRequest,
    },
    /// A redirect closed the previous exchange and started a new request.
    RequestRedirected {
        /// Correlation key (unchanged across the redirect chain).
        key: SourceKey,
        /// Response that produced the redirect.
        previous: NetworkResponse,
        /// The redirected request.
        request: NetworkRequest,
    },
    /// Response headers (and possibly a body) were observed.
    ResponseReceived {
        /// Correlation key.
        key: SourceKey,
        /// Response data observed so far.
        response: NetworkResponse,
    },
    /// A payload became available after the initial event.
    BodyUpdated {
        /// Correlation key.
        key: SourceKey,
        /// Which side of the exchange was updated.
        direction: BodyDirection,
        /// The payload.
        body: Body,
    },
    /// The exchange finished successfully.
    RequestCompleted {
        /// Correlation key.
        key: SourceKey,
        /// Final timings, when measured.
        timing: Option<TimingMetrics>,
        /// Final response, when the source reports it only at completion.
        response: Option<NetworkResponse>,
    },
    /// The exchange failed at the transport level.
    RequestFailed {
        /// Correlation key.
        key: SourceKey,
        /// Provider error text, kept verbatim for diagnosis.
        error: String,
        /// Final timings, when measured.
        timing: Option<TimingMetrics>,
    },
    /// The exchange was cancelled (navigation, target close, client abort).
    RequestAborted {
        /// Correlation key.
        key: SourceKey,
        /// Reason, when the provider reports one.
        reason: Option<String>,
    },
    /// A provider lost its connection; pending exchanges must be preserved.
    SourceDisconnected {
        /// Which provider disconnected.
        source: CaptureSource,
        /// Diagnostic reason.
        reason: String,
    },
    /// A provider reconnected and can emit events again.
    SourceReconnected {
        /// Which provider reconnected.
        source: CaptureSource,
    },
}

impl CaptureEvent {
    /// Correlation key, when the event belongs to one exchange.
    #[must_use]
    pub fn key(&self) -> Option<&SourceKey> {
        match self {
            Self::RequestStarted { key, .. }
            | Self::RequestRedirected { key, .. }
            | Self::ResponseReceived { key, .. }
            | Self::BodyUpdated { key, .. }
            | Self::RequestCompleted { key, .. }
            | Self::RequestFailed { key, .. }
            | Self::RequestAborted { key, .. } => Some(key),
            Self::SourceDisconnected { .. } | Self::SourceReconnected { .. } => None,
        }
    }

    /// Provider that produced the event.
    #[must_use]
    pub fn source(&self) -> CaptureSource {
        match self {
            Self::SourceDisconnected { source, .. } | Self::SourceReconnected { source } => *source,
            other => other.key().map_or(CaptureSource::Cdp, |key| key.source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExchangeState, HttpMethod, NetworkExchange, RequestUrl, Timestamp};

    #[test]
    fn keys_identify_the_provider_and_request() {
        let key = SourceKey::new(CaptureSource::Proxy, "7");
        assert_eq!(key.to_string(), "proxy:7");
    }

    #[test]
    fn lifecycle_events_expose_their_key_and_source() {
        let key = SourceKey::new(CaptureSource::Cdp, "1");
        let event = CaptureEvent::SourceDisconnected {
            source: CaptureSource::Proxy,
            reason: "closed".into(),
        };
        assert_eq!(event.key(), None);
        assert_eq!(event.source(), CaptureSource::Proxy);
        let event = CaptureEvent::RequestStarted {
            key: key.clone(),
            request: NetworkRequest::new(
                HttpMethod::parse("GET").unwrap(),
                RequestUrl::new("https://example.com/"),
                Timestamp::now(),
            ),
        };
        assert_eq!(event.key(), Some(&key));
        assert_eq!(event.source(), CaptureSource::Cdp);
    }

    #[test]
    fn a_pending_exchange_can_be_created_before_any_response() {
        let request = NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("https://example.com/"),
            Timestamp::now(),
        );
        let exchange =
            NetworkExchange::pending(SessionId::new(), CaptureSource::Proxy, request, None);
        assert_eq!(exchange.state, ExchangeState::Pending);
    }

    use crate::domain::SessionId;
}
