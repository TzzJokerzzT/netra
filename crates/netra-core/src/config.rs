//! Configuration shared by every adapter.
//!
//! Defaults are safe: capture limits are bounded, the proxy listens on loopback
//! only and the redaction policy masks credentials.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::domain::RedactionPolicy;

/// Default directory name inside the user data directory.
pub const APP_DIR_NAME: &str = "netra";

/// Size limits applied while capturing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureLimits {
    /// Maximum stored bytes per payload; larger payloads are truncated.
    pub max_body_bytes: usize,
    /// Maximum header lines retained per side.
    pub max_headers_per_side: usize,
    /// Maximum exchanges retained per session (oldest are dropped first).
    pub max_exchanges_per_session: u64,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_body_bytes: 1024 * 1024,
            max_headers_per_side: 200,
            max_exchanges_per_session: 50_000,
        }
    }
}

/// Proxy listener settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxySettings {
    /// Bind address. Loopback by default; exposing the proxy requires intent.
    pub bind_addr: String,
    /// Bind port.
    pub port: u16,
    /// Maximum simultaneous client connections.
    pub max_connections: usize,
    /// TLS interception is opt-in and requires a generated CA.
    pub intercept_tls: bool,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1".to_owned(),
            port: 8080,
            max_connections: 128,
            intercept_tls: false,
        }
    }
}

/// Top-level configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetraConfig {
    /// Directory holding the database, the certificate material and exports.
    ///
    /// When a configuration file sets `data_dir`, that path is honoured
    /// (relative paths resolve against the configuration file's directory).
    /// When it is omitted or empty, the file's directory is used. The value is
    /// never written back, so moving a data directory stays a deliberate act.
    #[serde(default, skip_serializing)]
    pub data_dir: PathBuf,
    /// Capture size limits.
    #[serde(default)]
    pub limits: CaptureLimits,
    /// Secret handling policy.
    #[serde(default)]
    pub redaction: RedactionPolicy,
    /// Proxy listener settings.
    #[serde(default)]
    pub proxy: ProxySettings,
}

impl NetraConfig {
    /// Default configuration rooted at the user's data directory.
    #[must_use]
    pub fn with_default_data_dir() -> Self {
        Self {
            data_dir: default_data_dir(),
            limits: CaptureLimits::default(),
            redaction: RedactionPolicy::default(),
            proxy: ProxySettings::default(),
        }
    }

    /// Path of the SQLite database.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("netra.db")
    }

    /// Path of the JSON configuration file.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    /// Loads configuration from `path`, falling back to defaults when the file
    /// does not exist yet.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or decoded.
    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut config: Self =
                    serde_json::from_str(&text).map_err(|error| ConfigError::Decode {
                        path: path.to_path_buf(),
                        reason: error.to_string(),
                    })?;
                let parent = path
                    .parent()
                    .map_or_else(default_data_dir, Path::to_path_buf);
                config.data_dir = if config.data_dir.as_os_str().is_empty() {
                    parent
                } else if config.data_dir.is_absolute() {
                    config.data_dir
                } else {
                    parent.join(config.data_dir)
                };
                // Configuration adds redaction rules; it never drops the
                // defaults that the security documentation promises.
                config.redaction = config.redaction.merged_with_defaults();
                Ok(config)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut config = Self::with_default_data_dir();
                config.data_dir = path
                    .parent()
                    .map_or_else(default_data_dir, Path::to_path_buf);
                Ok(config)
            }
            Err(error) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                reason: error.to_string(),
            }),
        }
    }

    /// Writes the configuration as pretty JSON, creating the directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| ConfigError::Io {
                path: parent.to_path_buf(),
                reason: error.to_string(),
            })?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| ConfigError::Decode {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
        fs::write(path, text).map_err(|error| ConfigError::Io {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })
    }

    /// Rejects settings that would make capture unsafe or useless.
    ///
    /// # Errors
    ///
    /// Returns a description of the first invalid setting.
    pub fn validate(&self) -> Result<(), String> {
        if self.limits.max_body_bytes == 0 {
            return Err("limits.max_body_bytes must be greater than zero".to_owned());
        }
        if self.limits.max_headers_per_side == 0 {
            return Err("limits.max_headers_per_side must be greater than zero".to_owned());
        }
        if self.proxy.port == 0 {
            return Err("proxy.port must be greater than zero".to_owned());
        }
        if self.proxy.bind_addr.trim().is_empty() {
            return Err("proxy.bind_addr must not be empty".to_owned());
        }
        if self.proxy.max_connections == 0 {
            return Err("proxy.max_connections must be greater than zero".to_owned());
        }
        Ok(())
    }
}

/// Configuration loading failures.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Filesystem failure.
    #[error("cannot access {path}: {reason}")]
    Io {
        /// Offending path.
        path: PathBuf,
        /// Underlying message.
        reason: String,
    },
    /// Invalid JSON or schema.
    #[error("cannot decode {path}: {reason}")]
    Decode {
        /// Offending path.
        path: PathBuf,
        /// Underlying message.
        reason: String,
    },
}

/// Resolves the default data directory:
/// `$XDG_DATA_HOME/netra`, else `$HOME/.local/share/netra`.
#[must_use]
pub fn default_data_dir() -> PathBuf {
    if let Some(xdg) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(xdg).join(APP_DIR_NAME);
    }
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(".local/share").join(APP_DIR_NAME);
    }
    PathBuf::from(".netra")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_loopback_and_bounded() {
        let config = NetraConfig::with_default_data_dir();
        assert_eq!(config.proxy.bind_addr, "127.0.0.1");
        assert!(!config.proxy.intercept_tls);
        assert!(config.limits.max_body_bytes > 0);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn invalid_limits_are_rejected() {
        let mut config = NetraConfig::with_default_data_dir();
        config.limits.max_body_bytes = 0;
        assert!(config.validate().is_err());
        config.limits.max_body_bytes = 1024;
        config.proxy.port = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = NetraConfig::with_default_data_dir();
        config.save(&path).unwrap();
        let loaded = NetraConfig::load_or_default(&path).unwrap();
        assert_eq!(loaded.limits, config.limits);
        assert_eq!(loaded.redaction.header_names, config.redaction.header_names);
        assert_eq!(loaded.data_dir, dir.path());
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = NetraConfig::load_or_default(&dir.path().join("absent.json")).unwrap();
        assert_eq!(loaded.proxy.port, ProxySettings::default().port);
    }

    #[test]
    fn corrupt_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{ not json").unwrap();
        assert!(NetraConfig::load_or_default(&path).is_err());
    }

    #[test]
    fn configured_data_dir_is_honoured_instead_of_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"data_dir":"/tmp/netra-explicit","proxy":{"bind_addr":"127.0.0.1","port":9000,"max_connections":4,"intercept_tls":false}}"#).unwrap();
        let config = NetraConfig::load_or_default(&path).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/tmp/netra-explicit"));
        assert_eq!(
            config.database_path(),
            PathBuf::from("/tmp/netra-explicit/netra.db")
        );
    }

    #[test]
    fn relative_data_dir_resolves_against_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"data_dir":"store"}"#).unwrap();
        let config = NetraConfig::load_or_default(&path).unwrap();
        assert_eq!(config.data_dir, dir.path().join("store"));
    }

    #[test]
    fn a_partial_config_no_longer_fails_on_the_missing_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{}").unwrap();
        let config = NetraConfig::load_or_default(&path).unwrap();
        assert_eq!(config.data_dir, dir.path());
    }

    #[test]
    fn saving_never_writes_the_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        NetraConfig::with_default_data_dir().save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("data_dir"),
            "data_dir must stay derived: {text}"
        );
    }

    #[test]
    fn configured_redaction_rules_add_to_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"redaction":{"header_names":["x-my-token"]}}"#).unwrap();
        let config = NetraConfig::load_or_default(&path).unwrap();
        assert!(
            config
                .redaction
                .header_names
                .iter()
                .any(|name| name == "x-my-token")
        );
        assert!(
            config
                .redaction
                .header_names
                .iter()
                .any(|name| name.eq_ignore_ascii_case("authorization")),
            "adding a rule must not drop the credential carriers"
        );
        assert!(
            config
                .redaction
                .query_params
                .iter()
                .any(|name| name == "token")
        );
    }

    #[test]
    fn database_path_lives_inside_the_data_dir() {
        let config = NetraConfig::with_default_data_dir();
        assert!(config.database_path().starts_with(&config.data_dir));
    }
}
