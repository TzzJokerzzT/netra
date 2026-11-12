//! `netra-application` — use cases, orchestration and the capture runtime.
//!
//! The TUI and the CLI talk only to this crate. It owns the capture pipeline
//! wiring (providers -> processor -> storage -> notifications) and never
//! renders or parses command-line arguments.

#![forbid(unsafe_code)]

pub mod context;
pub mod error;
pub mod export;
pub mod query;
pub mod runtime;
pub mod sessions;

pub use context::AppContext;
pub use error::{ApplicationError, ApplicationResult};
pub use query::QueryService;
pub use runtime::{AppEvent, CaptureRuntime};
pub use sessions::SessionService;
