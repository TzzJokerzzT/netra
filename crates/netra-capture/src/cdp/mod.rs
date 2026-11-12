//! Chrome DevTools Protocol capture.
//!
//! The adapter speaks CDP directly over a WebSocket: it discovers targets over
//! HTTP, attaches to page targets with flat sessions, enables the `Network`
//! domain and translates CDP payloads into [`CaptureEvent`]s. Everything the
//! inspector needs — correlation, limits, redaction, persistence — stays in the
//! shared pipeline; see ADR 0006 for why this is a small in-tree client instead
//! of a generated binding crate.

pub mod discovery;
pub mod mapping;
pub mod protocol;
pub mod provider;

pub use discovery::{CdpEndpoint, TargetInfo, browser_websocket_url, discover_targets};
pub use mapping::{CdpOutcome, TimingDraft, finalize_timing};
pub use provider::{CdpCaptureProvider, MAX_CAPTURED_BODY_BYTES};
