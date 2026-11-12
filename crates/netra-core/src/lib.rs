//! `netra-core` — domain model, value objects and ports.
//!
//! This crate is the innermost layer of `netra`. It must not depend on Ratatui,
//! SQLite, Tokio, CDP, the proxy implementation or any other infrastructure
//! framework: adapters depend on it, never the other way around.

#![forbid(unsafe_code)]

pub mod config;
pub mod domain;
pub mod error;
pub mod ports;

pub use error::{CaptureError, DomainError, RedactionError, StorageError, StorageResult};
