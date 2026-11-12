//! `netra-capture` — capture pipeline shared by every traffic source.
//!
//! The crate currently implements the source-agnostic half: correlation of
//! provider events into incremental exchange updates, with limits and
//! redaction applied before anything reaches storage.
//!
//! `cdp` implements the Chrome DevTools Protocol source (milestone M3) and
//! `proxy` the local HTTP/HTTPS proxy with optional TLS interception (M4).
//! Providers are the only code that knows a wire protocol; everything else
//! speaks [`CaptureEvent`].

#![forbid(unsafe_code)]

pub mod cdp;
pub mod demo;
pub mod processor;
pub mod proxy;

pub use processor::{CaptureProcessor, ProcessorDiagnostics};
