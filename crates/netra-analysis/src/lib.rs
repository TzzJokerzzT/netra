//! `netra-analysis` — pure presentation and inspection helpers.
//!
//! Everything here is deterministic and side-effect free: it turns captured
//! bytes into something readable and never executes or fetches anything.

#![forbid(unsafe_code)]

pub mod body_view;
pub mod errors;
pub mod headers;
pub mod json;
pub mod stats;
pub mod timing;

pub use body_view::BodyView;
pub use errors::{ErrorInsight, ErrorKind};
pub use stats::ExchangeStats;
pub use timing::{Waterfall, WaterfallSegment};
