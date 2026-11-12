//! Process exit codes.
//!
//! They are part of the CLI contract: scripts depend on them, so they are
//! defined here once and documented in `README.md`.

/// Exit code returned to the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// The command completed successfully.
    Success,
    /// The command failed (I/O, storage, capture).
    Failure,
    /// The requested entity does not exist.
    NotFound,
    /// The command was correct but the feature is not available yet.
    Unsupported,
}

impl ExitCode {
    /// Numeric code.
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Failure => 1,
            Self::NotFound => 4,
            Self::Unsupported => 3,
        }
    }
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(code: ExitCode) -> Self {
        Self::from(code.as_i32() as u8)
    }
}

/// Maps an application error to a process exit code.
#[must_use]
pub fn exit_code_for(error: &netra_application::ApplicationError) -> ExitCode {
    use netra_application::ApplicationError;
    match error {
        ApplicationError::NotFound(..) => ExitCode::NotFound,
        ApplicationError::Unsupported { .. } => ExitCode::Unsupported,
        _ => ExitCode::Failure,
    }
}
