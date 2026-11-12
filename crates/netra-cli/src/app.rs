//! Shared bootstrap: configuration, storage and application context.

use std::{path::PathBuf, sync::Arc};

use netra_application::AppContext;
use netra_core::{config::NetraConfig, ports::SystemClock};
use netra_storage::SqliteStore;

/// Everything a command needs, created once.
pub struct Bootstrap {
    /// Effective configuration.
    pub config: NetraConfig,
    /// Path of the configuration file in use.
    pub config_path: PathBuf,
    /// Shared application context.
    pub context: AppContext,
}

impl Bootstrap {
    /// Loads configuration, opens the database and applies migrations.
    ///
    /// # Errors
    ///
    /// Returns a message when configuration or storage cannot be prepared.
    pub fn load(config_override: Option<PathBuf>) -> Result<Self, String> {
        let config_path =
            config_override.unwrap_or_else(|| NetraConfig::with_default_data_dir().config_path());
        let config =
            NetraConfig::load_or_default(&config_path).map_err(|error| error.to_string())?;
        config.validate()?;
        let store =
            Arc::new(SqliteStore::open(config.database_path()).map_err(|error| error.to_string())?);
        Ok(Self {
            context: AppContext {
                sessions: store.clone(),
                exchanges: store,
                clock: Arc::new(SystemClock),
                config: config.clone(),
            },
            config,
            config_path,
        })
    }

    /// Directory exports are written to by default.
    #[must_use]
    pub fn exports_dir(&self) -> PathBuf {
        self.config.data_dir.join("exports")
    }
}

/// Initializes structured logging that never includes captured payloads.
pub fn init_logging() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_env("NETRA_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}
