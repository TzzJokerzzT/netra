//! `netra-capture` — capture pipeline shared by every traffic source.
//!
//! The crate currently implements the source-agnostic half: correlation of
//! provider events into incremental exchange updates, with limits and
//! redaction applied before anything reaches storage.
//!
//! The concrete providers (`cdp`, `proxy`) arrive in milestones M3 and M4; they
//! implement [`netra_core::ports::CaptureProvider`] and are the only code that
//! knows their wire protocol.

#![forbid(unsafe_code)]

pub mod demo;
pub mod processor;

pub use processor::{CaptureProcessor, ProcessorDiagnostics};
