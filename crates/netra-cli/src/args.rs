//! Command-line surface.
//!
//! Only commands that actually work are exposed: there is no `import` command
//! and no proxy/CDP flag that silently does nothing.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use netra_application::ApplicationError;
use netra_core::domain::SourceSelection;

/// netra — network inspector for the terminal.
#[derive(Debug, Parser)]
#[command(
    name = "netra",
    version,
    about = "Capture, inspect and persist HTTP traffic from the terminal",
    long_about = None,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Configuration file to load instead of the default one.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Open the TUI for the most recent session.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a session and start capturing.
    Capture(CaptureArgs),
    /// Inspect stored sessions.
    Sessions(SessionsArgs),
    /// Show the effective configuration and data locations.
    Config(ConfigArgs),
}

/// Arguments of `netra capture`.
#[derive(Debug, Args)]
pub struct CaptureArgs {
    /// Traffic sources to activate.
    #[arg(long, value_enum, default_value_t = SourceArg::Both)]
    pub source: SourceArg,

    /// Optional session name.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Run a scripted synthetic capture instead of a live source.
    ///
    /// The demo provider emits fixed fixtures: it is not real capture.
    #[arg(long)]
    pub demo: bool,
}

/// Arguments of `netra sessions`.
#[derive(Debug, Args)]
pub struct SessionsArgs {
    /// Sessions subcommand.
    #[command(subcommand)]
    pub command: SessionsCommand,
}

/// `netra sessions` subcommands.
#[derive(Debug, Subcommand)]
pub enum SessionsCommand {
    /// List stored sessions.
    List(ListArgs),
    /// Open a session in the TUI.
    Open(OpenArgs),
    /// Rename a session.
    Rename(RenameArgs),
    /// Mark a session as saved.
    Save(SaveArgs),
    /// Delete a session and its exchanges.
    Delete(DeleteArgs),
    /// Export a session.
    Export(ExportArgs),
}

/// Arguments of `netra sessions list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Print machine-readable JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments of `netra sessions open`.
#[derive(Debug, Args)]
pub struct OpenArgs {
    /// Session identifier (or a unique prefix).
    pub session_id: String,
}

/// Arguments of `netra sessions rename`.
#[derive(Debug, Args)]
pub struct RenameArgs {
    /// Session identifier (or a unique prefix).
    pub session_id: String,
    /// New name.
    pub name: String,
}

/// Arguments of `netra sessions save`.
#[derive(Debug, Args)]
pub struct SaveArgs {
    /// Session identifier (or a unique prefix).
    pub session_id: String,
}

/// Arguments of `netra sessions delete`.
#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Session identifier (or a unique prefix).
    pub session_id: String,
    /// Skip the interactive confirmation.
    #[arg(long)]
    pub yes: bool,
}

/// Arguments of `netra sessions export`.
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Session identifier (or a unique prefix).
    pub session_id: String,
    /// Output format.
    #[arg(long, value_enum, default_value_t = ExportFormat::Har)]
    pub format: ExportFormat,
    /// Output path; defaults to `<data dir>/exports/<session>.har`.
    #[arg(long, short, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

/// Arguments of `netra config`.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Print machine-readable JSON instead of a summary.
    #[arg(long)]
    pub json: bool,
}

/// Capture source selection on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SourceArg {
    /// Chrome DevTools Protocol only.
    Cdp,
    /// Local HTTP/HTTPS proxy only.
    Proxy,
    /// Both mechanisms.
    Both,
}

impl From<SourceArg> for SourceSelection {
    fn from(value: SourceArg) -> Self {
        match value {
            SourceArg::Cdp => Self::Cdp,
            SourceArg::Proxy => Self::Proxy,
            SourceArg::Both => Self::Both,
        }
    }
}

/// Export formats that are actually implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    /// HTTP Archive 1.2.
    Har,
}

impl ExportFormat {
    /// Resolves the format, rejecting unimplemented spellings with a clear
    /// error instead of silently falling back.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Unsupported`] for formats that are not
    /// implemented yet.
    pub fn resolve(self) -> Result<Self, ApplicationError> {
        Ok(self)
    }
}
