//! Incremental exchange changes.
//!
//! Capture is written progressively: every event produces one small update
//! instead of rewriting a whole aggregate. Adapters apply these updates inside
//! a transaction, so a crash loses at most the event in flight.

use serde::{Deserialize, Serialize};

use super::{
    body::Body,
    events::BodyDirection,
    exchange::{ExchangeState, NetworkExchange, NetworkResponse, TimingMetrics},
    ids::ExchangeId,
};

/// One atomic change to a persisted exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExchangeUpdate {
    /// A new exchange appeared: the request is visible before its response.
    Discovered(Box<NetworkExchange>),
    /// A redirect closed `previous_id` and started `next`.
    Redirected {
        /// Exchange closed by the redirect response.
        previous_id: ExchangeId,
        /// Final response of the closed exchange.
        previous: Box<NetworkResponse>,
        /// The redirected request, as a new exchange.
        next: Box<NetworkExchange>,
    },
    /// Response headers (and possibly body) arrived.
    Response {
        /// Target exchange.
        id: ExchangeId,
        /// Response data.
        response: Box<NetworkResponse>,
    },
    /// A payload became available after the initial event.
    Body {
        /// Target exchange.
        id: ExchangeId,
        /// Request or response side.
        direction: BodyDirection,
        /// Payload.
        body: Box<Body>,
    },
    /// Timings were measured.
    Timing {
        /// Target exchange.
        id: ExchangeId,
        /// Phase timings.
        timing: TimingMetrics,
    },
    /// The exchange reached a terminal state.
    State {
        /// Target exchange.
        id: ExchangeId,
        /// New state.
        state: ExchangeState,
        /// Failure or abort description.
        error: Option<String>,
    },
}

impl ExchangeUpdate {
    /// Identifier of the exchange this update targets.
    ///
    /// For [`ExchangeUpdate::Redirected`] this is the exchange that must be
    /// created next.
    #[must_use]
    pub fn exchange_id(&self) -> ExchangeId {
        match self {
            Self::Discovered(exchange) => exchange.id,
            Self::Redirected { next, .. } => next.id,
            Self::Response { id, .. }
            | Self::Body { id, .. }
            | Self::Timing { id, .. }
            | Self::State { id, .. } => *id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        CaptureSource, HttpMethod, NetworkRequest, RequestUrl, SessionId, Timestamp,
    };

    #[test]
    fn discovered_updates_identify_the_new_exchange() {
        let request = NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("https://example.com/"),
            Timestamp::now(),
        );
        let exchange =
            NetworkExchange::pending(SessionId::new(), CaptureSource::Proxy, request, None);
        let update = ExchangeUpdate::Discovered(Box::new(exchange.clone()));
        assert_eq!(update.exchange_id(), exchange.id);
    }
}
