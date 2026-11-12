//! Shared dependencies of every use case.

use std::{path::PathBuf, sync::Arc};

use netra_core::{
    config::NetraConfig,
    ports::{Clock, ExchangeRepository, SessionRepository},
};

/// Ports and configuration a use case needs.
#[derive(Clone)]
pub struct AppContext {
    /// Session persistence.
    pub sessions: Arc<dyn SessionRepository>,
    /// Exchange persistence.
    pub exchanges: Arc<dyn ExchangeRepository>,
    /// Time source.
    pub clock: Arc<dyn Clock>,
    /// Effective configuration.
    pub config: NetraConfig,
}

impl std::fmt::Debug for AppContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppContext")
            .field("data_dir", &self.config.data_dir)
            .finish_non_exhaustive()
    }
}

impl AppContext {
    /// Path of the SQLite database in use.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.config.database_path()
    }
}
