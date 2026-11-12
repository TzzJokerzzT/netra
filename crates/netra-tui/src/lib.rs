//! `netra-tui` — terminal presentation layer for the netra network inspector.
//!
//! The crate is a passive renderer. [`app::App`] holds nothing but view state:
//! the host hands it a [`app::Snapshot`] of summaries, asks whether the current
//! selection needs details through [`app::App::needs_detail`], and reacts to the
//! [`app::AppAction`] values produced by key presses. The app never reads the
//! clock, the database, the network or the environment.
//!
//! [`demo`] is a clearly synthetic data source used by the render tests and by
//! the CLI `--demo` mode.

#![forbid(unsafe_code)]

pub mod app;
pub mod components;
pub mod demo;

pub use app::{
    App, AppAction, AppState, ConfirmAction, ConfirmDialog, FocusPane, InspectorData, InspectorTab,
    SessionHeader, Snapshot,
};
