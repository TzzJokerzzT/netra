//! Domain model: entities, value objects, events and policies.
//!
//! The module re-exports the public surface so adapters import from
//! `netra_core::domain::*` instead of reaching into private submodules.

pub mod body;
pub mod delta;
pub mod events;
pub mod exchange;
pub mod filter;
pub mod headers;
pub mod http;
pub mod ids;
pub mod redaction;
pub mod session;
pub mod time;

pub use body::{Body, BodyData, OmitReason, human_bytes};
pub use delta::ExchangeUpdate;
pub use events::{BodyDirection, CaptureEvent, SourceKey};
pub use exchange::{
    CaptureSource, ExchangeState, NetworkExchange, NetworkRequest, NetworkResponse,
    SourceSelection, TimingMetrics,
};
pub use filter::{ExchangeFilter, ExchangeQuery, ExchangeSummary, StatusRange};
pub use headers::{HeaderEntry, HttpHeaders};
pub use http::{ContentType, HttpMethod, MediaKind, RequestUrl, StatusCode};
pub use ids::{ExchangeId, SessionId};
pub use redaction::{RedactionPolicy, RedactionReport};
pub use session::{CaptureSession, SessionStatus, SessionSummary};
pub use time::{DurationMs, Timestamp};
