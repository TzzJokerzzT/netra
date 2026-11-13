//! Self-update against the published GitHub release.
//!
//! Releases are published on `github.com/TzzJokerzzT/netra` for one platform
//! only, `x86_64-unknown-linux-musl`. The update path is split in two: every
//! decision (version comparison, release payload parsing, checksum
//! verification, install plan, cache staleness, notice text) is a pure function
//! unit-tested without a server, and the network and filesystem edges stay thin
//! around them.
//!
//! The declared trust limit is deliberate: the download is verified against the
//! SHA-256 published in the same release. That detects corruption and
//! truncation, not a compromised release, and the binary is not signed.

use std::{
    future::Future,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

use chrono::{DateTime, Utc};
use netra_core::config::NetraConfig;
use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;
use url::Url;

use crate::{args::UpdateArgs, exit::ExitCode};

/// Version of the running build.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Repository that publishes netra releases.
pub const REPOSITORY: &str = "TzzJokerzzT/netra";

/// The only platform netra is published for.
pub const TARGET_TRIPLE: &str = "x86_64-unknown-linux-musl";

/// Default GitHub API base.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

/// Environment variable that rebases the API onto a loopback server.
///
/// It exists for a manual smoke test of the update path; the release binary
/// never needs it.
pub const API_BASE_ENV: &str = "NETRA_UPDATE_API_BASE";

/// Environment variable the installation error points at.
pub const INSTALL_DIR_ENV: &str = "NETRA_INSTALL_DIR";

/// `Accept` header sent to the GitHub API.
const API_ACCEPT: &str = "application/vnd.github+json";

/// `Accept` header sent for a release asset.
const ASSET_ACCEPT: &str = "application/octet-stream";

/// Largest release document accepted, in bytes.
const MAX_RELEASE_BYTES: usize = 4 * 1024 * 1024;

/// Largest checksum sidecar accepted, in bytes.
const MAX_CHECKSUM_BYTES: usize = 8 * 1024;

/// Largest binary accepted, in bytes.
const MAX_BINARY_BYTES: usize = 256 * 1024 * 1024;

/// Deadline of the release query, connect and read included.
const API_TIMEOUT: Duration = Duration::from_secs(5);

/// Deadline of an asset download, connect and read included.
const ASSET_TIMEOUT: Duration = Duration::from_secs(120);

/// Deadline of the background startup refresh.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(2);

/// Redirect hops followed before giving up.
const MAX_REDIRECTS: usize = 3;

/// How long a cached answer stays fresh, in seconds.
const CACHE_MAX_AGE_SECS: i64 = 24 * 60 * 60;

/// Hosts a download redirect may land on.
const REDIRECT_HOSTS: [&str; 2] = ["github.com", "objects.githubusercontent.com"];

/// Update failures, each with one user-facing line.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// No release exists yet, so there is nothing to update to.
    #[error(
        "no release has been published for TzzJokerzzT/netra yet; there is nothing to update to"
    )]
    NoRelease,
    /// The endpoint could not be reached.
    #[error("cannot reach {url}: {reason}")]
    Network {
        /// URL that failed.
        url: String,
        /// Underlying message.
        reason: String,
    },
    /// The endpoint did not answer in time.
    #[error("{url} did not answer within {seconds}s; check your connection and try again")]
    Timeout {
        /// URL that timed out.
        url: String,
        /// Deadline in seconds.
        seconds: u64,
    },
    /// The endpoint answered with something the update path cannot use.
    #[error("unexpected answer from {url}: {reason}")]
    Response {
        /// URL that answered.
        url: String,
        /// What was wrong.
        reason: String,
    },
    /// The release does not publish the asset this platform needs.
    #[error("release {version} does not publish the {asset} asset")]
    MissingAsset {
        /// Published version.
        version: String,
        /// Missing asset name.
        asset: String,
    },
    /// The downloaded bytes are not the published bytes.
    #[error(
        "sha256 mismatch for {asset}: the release publishes {expected}, the download hashes to {actual}"
    )]
    ChecksumMismatch {
        /// Asset that was verified.
        asset: String,
        /// Digest published next to the release.
        expected: String,
        /// Digest of the download.
        actual: String,
    },
    /// The sidecar describes a different asset.
    #[error("the release .sha256 names {declared}, not {asset}")]
    ChecksumName {
        /// Asset that was requested.
        asset: String,
        /// Name declared by the sidecar.
        declared: String,
    },
    /// A document or identifier could not be parsed.
    #[error("cannot parse {name} `{value}`: {reason}")]
    Malformed {
        /// What could not be parsed.
        name: &'static str,
        /// Offending value.
        value: String,
        /// Why it is unusable.
        reason: String,
    },
    /// The API base override is not an allowlisted loopback address.
    #[error(
        "{env} must be http://127.0.0.1:<port> or http://localhost:<port>: {reason}",
        env = API_BASE_ENV
    )]
    ApiBase {
        /// Why the override was refused.
        reason: String,
    },
    /// The running executable could not be replaced.
    #[error(
        "cannot replace {}: {}; set {} to a writable directory and reinstall with install.sh",
        path.display(),
        reason,
        INSTALL_DIR_ENV
    )]
    Install {
        /// Path that was refused.
        path: PathBuf,
        /// Why the replacement failed.
        reason: String,
    },
    /// The update cache could not be written.
    #[error("cannot write the update cache {path}: {reason}")]
    Cache {
        /// Cache path.
        path: PathBuf,
        /// Why the write failed.
        reason: String,
    },
}

/// How the running build compares with a published release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionRelation {
    /// The release is newer than the running build.
    Newer,
    /// The release is the running build.
    Equal,
    /// The release is older than the running build.
    Older,
}

/// Name of the raw static binary asset.
#[must_use]
pub fn binary_asset_name() -> String {
    format!("netra-{TARGET_TRIPLE}")
}

/// Compares the running version with a published tag.
///
/// A leading `v` is accepted on both sides, because release tags are `vX.Y.Z`
/// while the crate version is `X.Y.Z`.
///
/// # Errors
///
/// Returns [`UpdateError::Malformed`] when either side is not a semantic
/// version.
pub fn relation(current: &str, latest: &str) -> Result<VersionRelation, UpdateError> {
    let current = parse_version(current)?;
    let latest = parse_version(latest)?;
    Ok(match latest.cmp(&current) {
        std::cmp::Ordering::Greater => VersionRelation::Newer,
        std::cmp::Ordering::Equal => VersionRelation::Equal,
        std::cmp::Ordering::Less => VersionRelation::Older,
    })
}

/// Parses a semantic version, tolerating the `v` of a release tag.
fn parse_version(text: &str) -> Result<Version, UpdateError> {
    let trimmed = text.trim();
    let bare = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(bare).map_err(|error| UpdateError::Malformed {
        name: "version",
        value: trimmed.to_owned(),
        reason: error.to_string(),
    })
}

/// One asset of a published release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReleaseAsset {
    /// File name inside the release.
    pub name: String,
    /// URL the asset is downloaded from.
    #[serde(default)]
    pub browser_download_url: String,
    /// Published size in bytes.
    #[serde(default)]
    pub size: u64,
}

/// The parts of GitHub's latest-release document the update path reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReleasePayload {
    /// Tag of the release, for example `v0.2.0`.
    #[serde(default)]
    pub tag_name: String,
    /// Human-readable release name.
    #[serde(default)]
    pub name: String,
    /// Whether the release is still a draft.
    #[serde(default)]
    pub draft: bool,
    /// Whether the release is a prerelease.
    #[serde(default)]
    pub prerelease: bool,
    /// Assets attached to the release.
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

/// What `netra update` will download and install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePlan {
    /// Published version, without the `v` prefix.
    pub version: String,
    /// Raw static binary of this platform.
    pub binary: ReleaseAsset,
    /// `sha256sum` sidecar for that binary.
    pub checksum: ReleaseAsset,
}

/// Plans an update from a GitHub latest-release document.
///
/// # Errors
///
/// Returns [`UpdateError::Malformed`] when the document is not JSON in the
/// expected shape and [`UpdateError::MissingAsset`] when the release does not
/// publish the static binary or its sidecar.
pub fn plan_from_release(text: &str) -> Result<UpdatePlan, UpdateError> {
    let payload: ReleasePayload =
        serde_json::from_str(text).map_err(|error| UpdateError::Malformed {
            name: "release document",
            value: "the latest release answer".to_owned(),
            reason: error.to_string(),
        })?;
    let version = payload
        .tag_name
        .trim()
        .strip_prefix('v')
        .unwrap_or(payload.tag_name.trim())
        .to_owned();
    let binary_name = binary_asset_name();
    let checksum_name = format!("{binary_name}.sha256");
    let binary = payload
        .assets
        .iter()
        .find(|asset| asset.name == binary_name)
        .cloned()
        .ok_or_else(|| UpdateError::MissingAsset {
            version: version.clone(),
            asset: binary_name,
        })?;
    let checksum = payload
        .assets
        .iter()
        .find(|asset| asset.name == checksum_name)
        .cloned()
        .ok_or_else(|| UpdateError::MissingAsset {
            version: version.clone(),
            asset: checksum_name,
        })?;
    Ok(UpdatePlan {
        version,
        binary,
        checksum,
    })
}

/// A parsed `sha256sum` sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checksum {
    /// Lowercase hex digest.
    pub digest: String,
    /// File name the sidecar was produced for.
    pub file_name: Option<String>,
}

/// Parses a `sha256sum` sidecar, which is one `<digest>  <file>` line.
///
/// # Errors
///
/// Returns [`UpdateError::Malformed`] when no line carries a 64 character hex
/// digest.
pub fn parse_checksum(text: &str) -> Result<Checksum, UpdateError> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(digest) = fields.next() else {
            continue;
        };
        let usable = digest.len() == 64 && digest.chars().all(|digit| digit.is_ascii_hexdigit());
        if !usable {
            return Err(UpdateError::Malformed {
                name: "sha256 sidecar",
                value: line.to_owned(),
                reason: "the first field is not a 64 character hex digest".to_owned(),
            });
        }
        return Ok(Checksum {
            digest: digest.to_ascii_lowercase(),
            file_name: fields.next().map(|name| {
                name.trim_start_matches('*')
                    .rsplit('/')
                    .next()
                    .unwrap_or(name)
                    .to_owned()
            }),
        });
    }
    Err(UpdateError::Malformed {
        name: "sha256 sidecar",
        value: text.trim().to_owned(),
        reason: "it carries no digest".to_owned(),
    })
}

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Verifies downloaded bytes against the published sidecar.
///
/// # Errors
///
/// Returns [`UpdateError::ChecksumName`] when the sidecar describes another
/// file and [`UpdateError::ChecksumMismatch`] when the bytes do not match.
pub fn verify_sha256(asset: &str, checksum: &Checksum, bytes: &[u8]) -> Result<(), UpdateError> {
    if let Some(declared) = &checksum.file_name
        && declared != asset
    {
        return Err(UpdateError::ChecksumName {
            asset: asset.to_owned(),
            declared: declared.clone(),
        });
    }
    let actual = sha256_hex(bytes);
    if actual != checksum.digest {
        return Err(UpdateError::ChecksumMismatch {
            asset: asset.to_owned(),
            expected: checksum.digest.clone(),
            actual,
        });
    }
    Ok(())
}

/// Candidate API endpoint, optionally rebased onto a loopback server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSource {
    base: Url,
    rebased: bool,
}

impl UpdateSource {
    /// Reads [`API_BASE_ENV`] and resolves the endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateError::ApiBase`] when the override is not an allowlisted
    /// loopback address.
    pub fn from_env() -> Result<Self, UpdateError> {
        let raw = std::env::var(API_BASE_ENV).ok();
        Self::resolve(raw.as_deref())
    }

    /// Resolves the endpoint from an optional override.
    ///
    /// The default is the public GitHub API over HTTPS. An override is accepted
    /// only as `http://127.0.0.1:<port>` or `http://localhost:<port>`, so a
    /// plaintext endpoint can never be used for anything but a local smoke
    /// test.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateError::ApiBase`] when the override is refused.
    pub fn resolve(override_base: Option<&str>) -> Result<Self, UpdateError> {
        let Some(raw) = override_base.map(str::trim).filter(|text| !text.is_empty()) else {
            let base = Url::parse(DEFAULT_API_BASE).map_err(|error| UpdateError::ApiBase {
                reason: error.to_string(),
            })?;
            return Ok(Self {
                base,
                rebased: false,
            });
        };
        let refused = |reason: &str| UpdateError::ApiBase {
            reason: format!("`{raw}` is refused: {reason}"),
        };
        let url = Url::parse(raw).map_err(|error| refused(&error.to_string()))?;
        if url.scheme() != "http" {
            return Err(refused("only plain http on loopback is allowed"));
        }
        if !matches!(url.host_str(), Some("127.0.0.1" | "localhost")) {
            return Err(refused("a non-loopback http endpoint must never be used"));
        }
        let Some(port) = url.port() else {
            return Err(refused("the port is missing"));
        };
        if !matches!(url.path(), "" | "/") {
            return Err(refused("the override must be a bare origin"));
        }
        let base = Url::parse(&format!(
            "http://{}:{port}",
            url.host_str().unwrap_or("127.0.0.1")
        ))
        .map_err(|error| refused(&error.to_string()))?;
        Ok(Self {
            base,
            rebased: true,
        })
    }

    /// URL of the latest-release document.
    #[must_use]
    pub fn latest_release_url(&self) -> String {
        format!(
            "{}/repos/{REPOSITORY}/releases/latest",
            self.base.as_str().trim_end_matches('/')
        )
    }

    /// Whether a redirect from `from` to `to` may be followed.
    #[must_use]
    pub fn redirect_allowed(&self, from: &Url, to: &Url) -> bool {
        if self.rebased {
            return to.scheme() == "http"
                && to.host_str() == self.base.host_str()
                && to.port() == self.base.port();
        }
        to.scheme() == "https"
            && (to.host_str() == from.host_str() || {
                to.host_str()
                    .is_some_and(|host| REDIRECT_HOSTS.contains(&host))
            })
    }

    /// URL an asset is downloaded from.
    ///
    /// A loopback override rebases the download onto the local server, so the
    /// smoke test never needs the public network.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateError::Malformed`] when the published URL has no usable
    /// file name.
    pub fn asset_url(&self, published: &str) -> Result<Url, UpdateError> {
        let parsed = Url::parse(published).map_err(|error| UpdateError::Malformed {
            name: "asset URL",
            value: published.to_owned(),
            reason: error.to_string(),
        })?;
        if !self.rebased {
            return Ok(parsed);
        }
        let name = parsed
            .path_segments()
            .and_then(Iterator::last)
            .filter(|segment| !segment.is_empty())
            .ok_or_else(|| UpdateError::Malformed {
                name: "asset URL",
                value: published.to_owned(),
                reason: "it has no file name".to_owned(),
            })?;
        Url::parse(&format!(
            "{}/download/{name}",
            self.base.as_str().trim_end_matches('/')
        ))
        .map_err(|error| UpdateError::Malformed {
            name: "asset URL",
            value: published.to_owned(),
            reason: error.to_string(),
        })
    }
}

/// A parsed HTTP/1.1 response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// Response headers, names lowercased.
    pub headers: Vec<(String, String)>,
    /// Body bytes, delimited by the declared framing.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Value of a header, when present.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Cached answer of the last successful release query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCache {
    /// When the query ran, RFC 3339.
    pub checked_at: String,
    /// Newest published version, without a `v` prefix.
    pub latest: String,
}

impl UpdateCache {
    /// Parses a cache document.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateError::Malformed`] when the document is not JSON in the
    /// expected shape.
    pub fn parse(text: &str) -> Result<Self, UpdateError> {
        serde_json::from_str(text).map_err(|error| UpdateError::Malformed {
            name: "update cache",
            value: text.trim().to_owned(),
            reason: error.to_string(),
        })
    }

    /// Serializes the documented JSON shape.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateError::Malformed`] when the cache cannot be encoded.
    pub fn to_json(&self) -> Result<String, UpdateError> {
        serde_json::to_string(self).map_err(|error| UpdateError::Malformed {
            name: "update cache",
            value: "checked_at/latest".to_owned(),
            reason: error.to_string(),
        })
    }
}

/// `true` when the cache is missing, unreadable or older than the window.
#[must_use]
pub fn is_stale(cache: Option<&UpdateCache>, now: DateTime<Utc>) -> bool {
    let Some(cache) = cache else {
        return true;
    };
    let Ok(checked_at) = DateTime::parse_from_rfc3339(&cache.checked_at) else {
        return true;
    };
    let age = now.signed_duration_since(checked_at.with_timezone(&Utc));
    age.num_seconds() >= CACHE_MAX_AGE_SECS
}

/// One-line notice for a newer cached release.
///
/// It is silent when the cached version cannot be compared or is not newer, so
/// a corrupt cache never produces a false promise.
#[must_use]
pub fn notice_for(current: &str, latest: &str) -> Option<String> {
    let latest = latest.trim().strip_prefix('v').unwrap_or(latest.trim());
    matches!(relation(current, latest), Ok(VersionRelation::Newer))
        .then(|| format!("⚠ netra {latest} disponible — corré `netra update`"))
}

/// What the startup check does, decided without touching files or the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupPlan {
    /// Line to print on stderr, if any.
    pub notice: Option<String>,
    /// Whether the cache must be refreshed in the background.
    pub refresh: bool,
}

/// Decides the startup behaviour from configuration and the cached answer.
#[must_use]
pub fn plan_startup(
    check_enabled: bool,
    cache: Option<&UpdateCache>,
    now: DateTime<Utc>,
    current: &str,
) -> StartupPlan {
    if !check_enabled {
        return StartupPlan {
            notice: None,
            refresh: false,
        };
    }
    StartupPlan {
        notice: cache.and_then(|cache| notice_for(current, &cache.latest)),
        refresh: is_stale(cache, now),
    }
}

/// Reads the cache, treating anything unreadable or corrupt as missing.
#[must_use]
pub fn read_cache(path: &Path) -> Option<UpdateCache> {
    let text = std::fs::read_to_string(path).ok()?;
    UpdateCache::parse(&text).ok()
}

/// Writes the cache atomically, with private permissions.
///
/// # Errors
///
/// Returns [`UpdateError::Cache`] when the directory or file cannot be written.
pub fn write_cache(path: &Path, cache: &UpdateCache) -> Result<(), UpdateError> {
    let text = cache.to_json()?;
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return Err(UpdateError::Cache {
            path: path.to_path_buf(),
            reason: error.to_string(),
        });
    }
    let temp = sibling_temp(path);
    let written = write_private(&temp, text.as_bytes()).and_then(|()| {
        // `sync_all` is already done by `write_private`; the rename is atomic.
        std::fs::rename(&temp, path)
    });
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(UpdateError::Cache {
            path: path.to_path_buf(),
            reason: error.to_string(),
        });
    }
    Ok(())
}

/// Replaces `target` with `bytes`, through a temporary file in the same
/// directory.
///
/// The running binary is never opened for writing: the new bytes are written
/// and synced next to it and then renamed over it, so an interrupted update
/// either leaves the old binary or the new one.
///
/// # Errors
///
/// Returns [`UpdateError::Install`] when the target is not a regular file or the
/// directory refuses the new file.
pub fn install_binary(target: &Path, bytes: &[u8]) -> Result<(), UpdateError> {
    match std::fs::symlink_metadata(target) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(install_error(
                target,
                "the target is not a regular file".to_owned(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(install_error(target, error.to_string())),
    }
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| install_error(target, "the target has no directory".to_owned()))?;
    let temp = sibling_temp(target);
    let written = write_executable(&temp, bytes)
        .and_then(|()| std::fs::rename(&temp, target))
        .map_err(|error| format!("cannot write next to {}: {error}", parent.display()));
    if let Err(reason) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(install_error(target, reason));
    }
    Ok(())
}

/// Prints the startup notice and refreshes a missing or stale cache in a
/// detached task.
///
/// Nothing happens at all when the check is disabled: no network, no notice and
/// no write. The refresh has its own short deadline, is silent on failure and
/// never delays the command that started it.
pub fn startup_check(config: &NetraConfig) {
    if !config.update.check {
        return;
    }
    let path = config.update_check_path();
    let cache = read_cache(&path);
    let plan = plan_startup(true, cache.as_ref(), Utc::now(), CURRENT_VERSION);
    if let Some(line) = plan.notice {
        eprintln!("{line}");
    }
    if plan.refresh {
        spawn_refresh(path);
    }
}

/// Runs `netra update`, printing the contract lines of the accepted mechanism.
///
/// # Errors
///
/// Returns the failure the caller reports on stderr with exit code 1.
pub async fn run(args: &UpdateArgs) -> Result<(), UpdateError> {
    let source = UpdateSource::from_env()?;
    let plan = latest_plan(&source).await?;
    let current = CURRENT_VERSION;
    let relation = relation(current, &plan.version)?;
    if args.check {
        println!("current: {current}");
        println!("latest:  {}", plan.version);
        return Ok(());
    }
    match relation {
        VersionRelation::Newer => println!("netra {current} → {} available", plan.version),
        VersionRelation::Equal if args.force => {
            println!("netra {current} → {} (reinstalling)", plan.version);
        }
        VersionRelation::Equal | VersionRelation::Older => {
            println!("netra {current} (latest: {}) — up to date", plan.version);
            return Ok(());
        }
    }
    let expected = fetch_checksum(&source, &plan).await?;
    let bytes = fetch_binary(&source, &plan).await?;
    verify_sha256(&plan.binary.name, &expected, &bytes)?;
    println!("sha256: verificado contra el .sha256 del release");
    let target = running_executable()?;
    install_binary(&target, &bytes)?;
    println!("installed: {} (replacing {current})", target.display());
    Ok(())
}

/// Reports an update failure on stderr and maps it to the exit code.
#[must_use]
pub fn report(error: &UpdateError) -> ExitCode {
    eprintln!("netra: {error}");
    ExitCode::Failure
}

/// Queries the latest release and plans the update.
///
/// # Errors
///
/// Returns the failure the caller reports on stderr with exit code 1.
pub async fn latest_plan(source: &UpdateSource) -> Result<UpdatePlan, UpdateError> {
    let url = Url::parse(&source.latest_release_url()).map_err(|error| UpdateError::Malformed {
        name: "API URL",
        value: source.latest_release_url(),
        reason: error.to_string(),
    })?;
    let response = with_deadline(
        &url,
        API_TIMEOUT,
        get(source, &url, API_ACCEPT, MAX_RELEASE_BYTES),
    )
    .await?;
    check_status(&url, &response)?;
    let text = String::from_utf8(response.body).map_err(|error| UpdateError::Response {
        url: url.to_string(),
        reason: format!("the release document is not UTF-8: {error}"),
    })?;
    plan_from_release(&text)
}

/// Downloads and parses the release's `.sha256` sidecar.
///
/// # Errors
///
/// Returns the failure the caller reports on stderr with exit code 1.
pub async fn fetch_checksum(
    source: &UpdateSource,
    plan: &UpdatePlan,
) -> Result<Checksum, UpdateError> {
    let url = source.asset_url(&plan.checksum.browser_download_url)?;
    let response = with_deadline(
        &url,
        ASSET_TIMEOUT,
        get(source, &url, ASSET_ACCEPT, MAX_CHECKSUM_BYTES),
    )
    .await?;
    check_status(&url, &response)?;
    let text = String::from_utf8(response.body).map_err(|error| UpdateError::Response {
        url: url.to_string(),
        reason: format!("the sidecar is not UTF-8: {error}"),
    })?;
    parse_checksum(&text)
}

/// Downloads the raw static binary of a release.
///
/// # Errors
///
/// Returns the failure the caller reports on stderr with exit code 1.
pub async fn fetch_binary(
    source: &UpdateSource,
    plan: &UpdatePlan,
) -> Result<Vec<u8>, UpdateError> {
    let url = source.asset_url(&plan.binary.browser_download_url)?;
    let response = with_deadline(
        &url,
        ASSET_TIMEOUT,
        get(source, &url, ASSET_ACCEPT, MAX_BINARY_BYTES),
    )
    .await?;
    check_status(&url, &response)?;
    Ok(response.body)
}

/// Maps a response status to the failure it means.
///
/// # Errors
///
/// Returns [`UpdateError::NoRelease`] for a 404 and
/// [`UpdateError::Response`] for any other non-success status.
pub fn check_status(url: &Url, response: &HttpResponse) -> Result<(), UpdateError> {
    match response.status {
        200..=299 => Ok(()),
        404 => Err(UpdateError::NoRelease),
        status => Err(UpdateError::Response {
            url: url.to_string(),
            reason: format!("the server answered HTTP {status}"),
        }),
    }
}

/// Parses a response, or returns `None` while it is still incomplete.
fn parse_response(url: &str, raw: &[u8]) -> Result<Option<HttpResponse>, UpdateError> {
    let Some(end) = find_head_end(raw) else {
        return Ok(None);
    };
    let head = std::str::from_utf8(&raw[..end]).map_err(|error| UpdateError::Response {
        url: url.to_owned(),
        reason: format!("the response head is not UTF-8: {error}"),
    })?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = parse_status_line(status_line).ok_or_else(|| UpdateError::Response {
        url: url.to_owned(),
        reason: format!("no usable status line in `{status_line}`"),
    })?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let body = &raw[end + 4..];
    let chunked = headers.iter().any(|(name, value)| {
        name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked")
    });
    if chunked {
        return match decode_chunked(body) {
            Ok(Some(decoded)) => Ok(Some(HttpResponse {
                status,
                headers,
                body: decoded,
            })),
            Ok(None) => Ok(None),
            Err(reason) => Err(UpdateError::Response {
                url: url.to_owned(),
                reason,
            }),
        };
    }
    let Some(length) = declared_length(&headers) else {
        return Err(UpdateError::Response {
            url: url.to_owned(),
            reason: "the response declared neither Content-Length nor chunked framing".to_owned(),
        });
    };
    if body.len() < length {
        return Ok(None);
    }
    Ok(Some(HttpResponse {
        status,
        headers,
        body: body[..length].to_vec(),
    }))
}

/// Decodes a chunked body; `None` while more bytes are needed.
fn decode_chunked(raw: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let mut body = Vec::new();
    let mut rest = raw;
    loop {
        let Some(line_end) = rest.windows(2).position(|window| window == b"\r\n") else {
            return Ok(None);
        };
        let line = std::str::from_utf8(&rest[..line_end]).map_err(|error| error.to_string())?;
        let size_text = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("invalid chunk size `{size_text}`"))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            let terminated =
                rest.starts_with(b"\r\n") || rest.windows(4).any(|window| window == b"\r\n\r\n");
            return Ok(terminated.then_some(body));
        }
        if rest.len() < size + 2 {
            return Ok(None);
        }
        body.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

/// Position of the header terminator in a raw response.
fn find_head_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Status code of an HTTP response line.
fn parse_status_line(line: &str) -> Option<u16> {
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

/// `Content-Length` of a response, when declared.
fn declared_length(headers: &[(String, String)]) -> Option<usize> {
    headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.trim().parse().ok())
}

/// HTTP/1.1 GET request head.
fn request_head(host: &str, port: u16, scheme: &str, path: &str, accept: &str) -> String {
    let authority = if (scheme == "https" && port == 443) || (scheme == "http" && port == 80) {
        host.to_owned()
    } else {
        format!("{host}:{port}")
    };
    format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nAccept: {accept}\r\nUser-Agent: netra/{CURRENT_VERSION}\r\nConnection: close\r\n\r\n"
    )
}

/// Object-safe stream the request is spoken over.
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Boxed connection, TLS or plain loopback.
type BoxedStream = Box<dyn Stream>;

/// Process-wide TLS client configuration.
fn tls_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }))
}

/// One request/response exchange, without redirect handling.
async fn exchange(url: &Url, accept: &str, max_bytes: usize) -> Result<HttpResponse, UpdateError> {
    let host = url
        .host_str()
        .ok_or_else(|| response_error(url, "the URL has no host"))?
        .to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| response_error(url, "the URL has no port"))?;
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|error| UpdateError::Network {
            url: url.to_string(),
            reason: error.to_string(),
        })?;
    tcp.set_nodelay(true).ok();
    let mut stream: BoxedStream = match url.scheme() {
        "https" => {
            let server_name =
                ServerName::try_from(host.clone()).map_err(|error| UpdateError::Malformed {
                    name: "TLS server name",
                    value: host.clone(),
                    reason: error.to_string(),
                })?;
            let connector = TlsConnector::from(tls_config());
            let tls = connector.connect(server_name, tcp).await.map_err(|error| {
                UpdateError::Network {
                    url: url.to_string(),
                    reason: format!("TLS handshake failed: {error}"),
                }
            })?;
            Box::new(tls)
        }
        // Plain HTTP is only reachable for the loopback smoke-test override.
        "http" => Box::new(tcp),
        scheme => {
            return Err(response_error(
                url,
                &format!("unsupported scheme `{scheme}`"),
            ));
        }
    };
    let mut path = url.path().to_owned();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let head = request_head(&host, port, url.scheme(), &path, accept);
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|error| UpdateError::Network {
            url: url.to_string(),
            reason: error.to_string(),
        })?;
    stream.flush().await.ok();
    let mut raw: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| UpdateError::Network {
                url: url.to_string(),
                reason: error.to_string(),
            })?;
        if read == 0 {
            return match parse_response(url.as_str(), &raw)? {
                Some(response) => Ok(response),
                None => Err(response_error(
                    url,
                    "the connection closed before the response was complete",
                )),
            };
        }
        raw.extend_from_slice(&chunk[..read]);
        if raw.len() > max_bytes.saturating_add(64 * 1024) {
            return Err(response_error(url, "the response is larger than allowed"));
        }
        if let Some(response) = parse_response(url.as_str(), &raw)? {
            return Ok(response);
        }
    }
}

/// Fetches `url`, following at most [`MAX_REDIRECTS`] allowlisted redirects.
async fn get(
    source: &UpdateSource,
    url: &Url,
    accept: &str,
    max_bytes: usize,
) -> Result<HttpResponse, UpdateError> {
    let mut current = url.clone();
    let mut hops = 0;
    loop {
        let response = exchange(&current, accept, max_bytes).await?;
        if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            if hops >= MAX_REDIRECTS {
                return Err(response_error(
                    &current,
                    &format!("more than {MAX_REDIRECTS} redirects"),
                ));
            }
            let location = response
                .header("location")
                .ok_or_else(|| response_error(&current, "a redirect without a Location header"))?;
            let next = current.join(location).map_err(|error| {
                response_error(&current, &format!("unusable Location: {error}"))
            })?;
            if !source.redirect_allowed(&current, &next) {
                return Err(response_error(
                    &current,
                    &format!("refusing to follow a redirect to {next}"),
                ));
            }
            hops += 1;
            current = next;
            continue;
        }
        return Ok(response);
    }
}

/// Applies a total deadline to one HTTP call, connect and read included.
async fn with_deadline(
    url: &Url,
    limit: Duration,
    work: impl Future<Output = Result<HttpResponse, UpdateError>>,
) -> Result<HttpResponse, UpdateError> {
    match tokio::time::timeout(limit, work).await {
        Ok(result) => result,
        Err(_) => Err(UpdateError::Timeout {
            url: url.to_string(),
            seconds: limit.as_secs(),
        }),
    }
}

/// Refreshes the cache in a detached task, silently and with its own deadline.
fn spawn_refresh(path: PathBuf) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        let _ = tokio::time::timeout(REFRESH_TIMEOUT, refresh_cache(&path)).await;
    });
}

/// Queries the latest release and stores the answer in the cache.
async fn refresh_cache(path: &Path) -> Result<(), UpdateError> {
    let source = UpdateSource::from_env()?;
    let plan = latest_plan(&source).await?;
    let cache = UpdateCache {
        checked_at: Utc::now().to_rfc3339(),
        latest: plan.version,
    };
    write_cache(path, &cache)
}

/// Absolute path of the running executable.
fn running_executable() -> Result<PathBuf, UpdateError> {
    let exe = std::env::current_exe().map_err(|error| UpdateError::Install {
        path: PathBuf::from("netra"),
        reason: format!("cannot locate the running binary: {error}"),
    })?;
    std::fs::canonicalize(&exe).map_err(|error| UpdateError::Install {
        path: exe.clone(),
        reason: format!("cannot resolve the running binary: {error}"),
    })
}

/// Builds an installation failure that names the path and the escape hatch.
fn install_error(target: &Path, reason: String) -> UpdateError {
    UpdateError::Install {
        path: target.to_path_buf(),
        reason,
    }
}

/// Builds a response failure for a URL.
fn response_error(url: &Url, reason: &str) -> UpdateError {
    UpdateError::Response {
        url: url.to_string(),
        reason: reason.to_owned(),
    }
}

/// A hidden temporary file next to `path`.
fn sibling_temp(path: &Path) -> PathBuf {
    let name = path.file_name().map_or_else(
        || "netra".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    path.with_file_name(format!(".{name}.{}.{nanos}.tmp", std::process::id()))
}

/// Writes a fresh file with `0600`, then syncs it.
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut file = create_new_file(path, 0o600)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Writes a fresh executable file with `0755`, then syncs it.
fn write_executable(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut file = create_new_file(path, 0o755)?;
    file.write_all(contents)?;
    file.set_permissions(executable_permissions())?;
    file.sync_all()
}

/// Permissions a replaced binary must carry.
fn executable_permissions() -> std::fs::Permissions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::Permissions::from_mode(0o755)
    }
    #[cfg(not(unix))]
    {
        std::fs::Permissions::new()
    }
}

/// Creates a file that must not exist yet, with the given mode.
fn create_new_file(path: &Path, mode: u32) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One recorded `GET /releases/latest` answer, trimmed to the fields the
    /// update path reads but with the real asset names and URLs.
    const RELEASE_PAYLOAD: &str = r#"{
      "url": "https://api.github.com/repos/TzzJokerzzT/netra/releases/178090123",
      "assets_url": "https://api.github.com/repos/TzzJokerzzT/netra/releases/178090123/assets",
      "html_url": "https://github.com/TzzJokerzzT/netra/releases/tag/v0.2.0",
      "id": 178090123,
      "tag_name": "v0.2.0",
      "target_commitish": "main",
      "name": "netra 0.2.0",
      "draft": false,
      "prerelease": false,
      "created_at": "2025-06-01T10:00:00Z",
      "published_at": "2025-06-01T10:00:05Z",
      "assets": [
        {
          "url": "https://api.github.com/repos/TzzJokerzzT/netra/releases/assets/1",
          "id": 1,
          "name": "netra-x86_64-unknown-linux-musl",
          "label": "",
          "content_type": "application/octet-stream",
          "state": "uploaded",
          "size": 5242880,
          "download_count": 12,
          "browser_download_url": "https://github.com/TzzJokerzzT/netra/releases/download/v0.2.0/netra-x86_64-unknown-linux-musl"
        },
        {
          "url": "https://api.github.com/repos/TzzJokerzzT/netra/releases/assets/2",
          "id": 2,
          "name": "netra-x86_64-unknown-linux-musl.sha256",
          "content_type": "text/plain",
          "state": "uploaded",
          "size": 97,
          "download_count": 9,
          "browser_download_url": "https://github.com/TzzJokerzzT/netra/releases/download/v0.2.0/netra-x86_64-unknown-linux-musl.sha256"
        },
        {
          "url": "https://api.github.com/repos/TzzJokerzzT/netra/releases/assets/3",
          "id": 3,
          "name": "netra-x86_64-unknown-linux-musl.tar.gz",
          "content_type": "application/gzip",
          "state": "uploaded",
          "size": 2048000,
          "download_count": 4,
          "browser_download_url": "https://github.com/TzzJokerzzT/netra/releases/download/v0.2.0/netra-x86_64-unknown-linux-musl.tar.gz"
        },
        {
          "url": "https://api.github.com/repos/TzzJokerzzT/netra/releases/assets/4",
          "id": 4,
          "name": "netra-x86_64-unknown-linux-musl.tar.gz.sha256",
          "content_type": "text/plain",
          "state": "uploaded",
          "size": 103,
          "download_count": 2,
          "browser_download_url": "https://github.com/TzzJokerzzT/netra/releases/download/v0.2.0/netra-x86_64-unknown-linux-musl.tar.gz.sha256"
        }
      ],
      "tarball_url": "https://api.github.com/repos/TzzJokerzzT/netra/tarball/v0.2.0",
      "zipball_url": "https://api.github.com/repos/TzzJokerzzT/netra/zipball/v0.2.0",
      "body": "See docs/release.md."
    }"#;

    #[test]
    fn version_comparison_reports_newer_equal_and_older() {
        assert_eq!(
            relation("0.1.0", "0.2.0").unwrap(),
            VersionRelation::Newer,
            "a greater release is an update"
        );
        assert_eq!(relation("0.2.0", "0.2.0").unwrap(), VersionRelation::Equal);
        assert_eq!(
            relation("0.3.0", "0.2.0").unwrap(),
            VersionRelation::Older,
            "a local build ahead of the release is not downgraded"
        );
        assert_eq!(
            relation("v0.1.0", "v0.2.0").unwrap(),
            VersionRelation::Newer,
            "release tags carry a v prefix"
        );
        assert_eq!(relation("0.1.0", "0.1.1").unwrap(), VersionRelation::Newer);
        assert_eq!(relation("0.1.0", "1.0.0").unwrap(), VersionRelation::Newer);
        assert!(
            matches!(
                relation("0.1.0", "not a version"),
                Err(UpdateError::Malformed { .. })
            ),
            "an unparseable release version must be reported, not guessed"
        );
    }

    #[test]
    fn prerelease_versions_order_below_their_release() {
        assert_eq!(
            relation("0.2.0-rc.1", "0.2.0").unwrap(),
            VersionRelation::Newer,
            "the final release is newer than its candidate"
        );
        assert_eq!(
            relation("0.2.0", "0.2.0-rc.1").unwrap(),
            VersionRelation::Older,
            "a candidate never replaces a final release"
        );
        assert_eq!(
            relation("0.2.0-alpha", "0.2.0-beta").unwrap(),
            VersionRelation::Newer,
            "prerelease identifiers order alphabetically"
        );
        assert_eq!(
            relation("0.2.0-alpha", "0.2.0-alpha.1").unwrap(),
            VersionRelation::Newer,
            "more prerelease identifiers win when the prefix matches"
        );
        assert_eq!(
            relation("0.2.0-alpha.1", "0.2.0-alpha").unwrap(),
            VersionRelation::Older,
            "a shorter prerelease is older, so it is never installed over a longer one"
        );
    }

    #[test]
    fn a_recorded_release_payload_plans_the_static_binary() {
        let plan = plan_from_release(RELEASE_PAYLOAD).unwrap();
        assert_eq!(plan.version, "0.2.0", "the v prefix is not installed");
        assert_eq!(plan.binary.name, "netra-x86_64-unknown-linux-musl");
        assert_eq!(plan.checksum.name, "netra-x86_64-unknown-linux-musl.sha256");
        assert!(
            plan.binary
                .browser_download_url
                .ends_with("/releases/download/v0.2.0/netra-x86_64-unknown-linux-musl"),
            "the raw binary is chosen, not the tarball: {}",
            plan.binary.browser_download_url
        );
        assert_eq!(plan.binary.size, 5_242_880);
        assert!(
            !plan.binary.name.ends_with(".tar.gz"),
            "S5 publishes a single raw asset"
        );
    }

    #[test]
    fn a_release_without_the_static_binary_is_rejected() {
        let payload = r#"{"tag_name":"v0.2.0","assets":[
            {"name":"netra-x86_64-unknown-linux-musl.tar.gz",
             "browser_download_url":"https://github.com/x/y.tar.gz"},
            {"name":"netra-x86_64-unknown-linux-musl.tar.gz.sha256",
             "browser_download_url":"https://github.com/x/y.tar.gz.sha256"}]}"#;
        match plan_from_release(payload) {
            Err(UpdateError::MissingAsset { asset, version }) => {
                assert_eq!(asset, "netra-x86_64-unknown-linux-musl");
                assert_eq!(version, "0.2.0");
            }
            other => panic!("expected a missing asset error, got {other:?}"),
        }
    }

    #[test]
    fn a_payload_that_is_not_a_release_document_is_rejected() {
        assert!(matches!(
            plan_from_release("<html>404</html>"),
            Err(UpdateError::Malformed { .. })
        ));
        assert!(matches!(
            plan_from_release(r#"{"assets":[{"name":"x"}]}"#),
            Err(UpdateError::MissingAsset { .. })
        ));
    }

    #[test]
    fn sha256_sidecar_is_parsed_and_a_mismatch_is_rejected() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let bytes = b"static netra binary";
        let digest = sha256_hex(bytes);
        let text = format!("{digest}  {}\n", binary_asset_name());
        let checksum = parse_checksum(&text).unwrap();
        assert_eq!(checksum.digest, digest);
        assert_eq!(
            checksum.file_name.as_deref(),
            Some("netra-x86_64-unknown-linux-musl")
        );
        assert!(verify_sha256(&binary_asset_name(), &checksum, bytes).is_ok());

        match verify_sha256(&binary_asset_name(), &checksum, b"truncated download") {
            Err(UpdateError::ChecksumMismatch {
                expected, actual, ..
            }) => {
                assert_eq!(expected, digest);
                assert_ne!(actual, digest);
            }
            other => panic!("expected a checksum mismatch, got {other:?}"),
        }
        assert!(
            matches!(
                verify_sha256("netra-x86_64-unknown-linux-musl.tar.gz", &checksum, bytes),
                Err(UpdateError::ChecksumName { .. })
            ),
            "a sidecar for another asset must not verify this one"
        );

        let upper = format!(
            "{}  netra-x86_64-unknown-linux-musl",
            digest.to_ascii_uppercase()
        );
        assert_eq!(parse_checksum(&upper).unwrap().digest, digest);
        assert!(parse_checksum("").is_err());
        assert!(parse_checksum("not-a-digest  netra\n").is_err());
        assert!(
            parse_checksum("abc  netra\n").is_err(),
            "a short digest is not a sha256"
        );
    }

    #[test]
    fn only_a_loopback_http_override_is_accepted() {
        let source = UpdateSource::resolve(None).unwrap();
        assert_eq!(
            source.latest_release_url(),
            "https://api.github.com/repos/TzzJokerzzT/netra/releases/latest"
        );

        for accepted in [
            "http://127.0.0.1:8099",
            "http://127.0.0.1:8099/",
            "http://localhost:8099",
        ] {
            let source = UpdateSource::resolve(Some(accepted)).unwrap();
            assert_eq!(
                source.latest_release_url(),
                format!(
                    "{}/repos/TzzJokerzzT/netra/releases/latest",
                    accepted.trim_end_matches('/')
                )
            );
        }

        for refused in [
            "http://api.github.com",
            "http://api.github.com:80",
            "http://example.com:8099",
            "http://10.0.0.1:8099",
            "http://0.0.0.0:8099",
            "http://192.168.1.20:8099",
            "http://127.0.0.1",
            "http://[::1]:8099",
            "https://127.0.0.1:8099",
            "ftp://127.0.0.1:8099",
            "127.0.0.1:8099",
            "not a url",
        ] {
            assert!(
                matches!(
                    UpdateSource::resolve(Some(refused)),
                    Err(UpdateError::ApiBase { .. })
                ),
                "`{refused}` must be refused"
            );
        }
    }

    #[test]
    fn loopback_override_rebases_asset_downloads() {
        let published = "https://github.com/TzzJokerzzT/netra/releases/download/v0.2.0/netra-x86_64-unknown-linux-musl";
        let public = UpdateSource::resolve(None).unwrap();
        assert_eq!(
            public.asset_url(published).unwrap().as_str(),
            published,
            "the published URL is used as is when nothing is overridden"
        );
        let local = UpdateSource::resolve(Some("http://127.0.0.1:8099")).unwrap();
        assert_eq!(
            local.asset_url(published).unwrap().as_str(),
            "http://127.0.0.1:8099/download/netra-x86_64-unknown-linux-musl"
        );
    }

    #[test]
    fn redirects_stay_on_the_release_hosts() {
        let public = UpdateSource::resolve(None).unwrap();
        let from =
            Url::parse("https://github.com/TzzJokerzzT/netra/releases/download/v0.2.0/x").unwrap();
        let asset = Url::parse(
            "https://objects.githubusercontent.com/github-production-release-asset/1?X-Amz=x",
        )
        .unwrap();
        let same =
            Url::parse("https://github.com/TzzJokerzzT/netra/releases/download/v0.2.0/y").unwrap();
        let other = Url::parse("https://attacker.example/netra").unwrap();
        let plain = Url::parse("http://objects.githubusercontent.com/netra").unwrap();
        assert!(public.redirect_allowed(&from, &asset));
        assert!(public.redirect_allowed(&from, &same));
        assert!(!public.redirect_allowed(&from, &other));
        assert!(!public.redirect_allowed(&from, &plain));

        let local = UpdateSource::resolve(Some("http://127.0.0.1:8099")).unwrap();
        let local_from = Url::parse("http://127.0.0.1:8099/download/netra").unwrap();
        let local_same = Url::parse("http://127.0.0.1:8099/download/netra-2").unwrap();
        assert!(local.redirect_allowed(&local_from, &local_same));
        assert!(!local.redirect_allowed(&local_from, &other));
        assert!(
            !local.redirect_allowed(&local_from, &asset),
            "a loopback smoke test never reaches a public host"
        );
    }

    #[test]
    fn cache_staleness_uses_a_24_hour_window() {
        let now = DateTime::parse_from_rfc3339("2025-06-02T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(is_stale(None, now), "a missing cache needs a refresh");

        let fresh = UpdateCache {
            checked_at: "2025-06-02T11:00:00Z".to_owned(),
            latest: "0.2.0".to_owned(),
        };
        assert!(!is_stale(Some(&fresh), now), "an hour old answer is fresh");

        let almost = UpdateCache {
            checked_at: "2025-06-01T12:00:01Z".to_owned(),
            latest: "0.2.0".to_owned(),
        };
        assert!(!is_stale(Some(&almost), now));

        let old = UpdateCache {
            checked_at: "2025-06-01T11:59:59Z".to_owned(),
            latest: "0.2.0".to_owned(),
        };
        assert!(is_stale(Some(&old), now), "a day old answer is stale");

        let corrupt = UpdateCache {
            checked_at: "yesterday".to_owned(),
            latest: "0.2.0".to_owned(),
        };
        assert!(is_stale(Some(&corrupt), now), "an unreadable date is stale");
    }

    #[test]
    fn the_notice_only_appears_for_a_newer_cached_release() {
        assert_eq!(
            notice_for("0.1.0", "0.2.0").as_deref(),
            Some("⚠ netra 0.2.0 disponible — corré `netra update`")
        );
        assert_eq!(notice_for("0.2.0", "0.2.0"), None);
        assert_eq!(notice_for("0.3.0", "0.2.0"), None);
        assert_eq!(
            notice_for("0.1.0", "v0.2.0").as_deref(),
            Some("⚠ netra 0.2.0 disponible — corré `netra update`")
        );
        assert_eq!(
            notice_for("0.1.0", "garbage"),
            None,
            "an unreadable cached version must stay silent"
        );
    }

    #[test]
    fn startup_plan_is_silent_when_the_check_is_disabled() {
        let now = Utc::now();
        let cache = UpdateCache {
            checked_at: "2020-01-01T00:00:00Z".to_owned(),
            latest: "0.2.0".to_owned(),
        };
        let plan = plan_startup(false, Some(&cache), now, "0.1.0");
        assert_eq!(
            plan,
            StartupPlan {
                notice: None,
                refresh: false
            }
        );

        let plan = plan_startup(true, Some(&cache), now, "0.1.0");
        assert_eq!(
            plan.notice.as_deref(),
            Some("⚠ netra 0.2.0 disponible — corré `netra update`")
        );
        assert!(plan.refresh, "a stale cache is refreshed in the background");

        let fresh = UpdateCache {
            checked_at: now.to_rfc3339(),
            latest: "0.2.0".to_owned(),
        };
        let plan = plan_startup(true, Some(&fresh), now, "0.1.0");
        assert!(plan.notice.is_some());
        assert!(!plan.refresh, "a fresh cache is not refreshed again");

        let current = UpdateCache {
            checked_at: now.to_rfc3339(),
            latest: "0.1.0".to_owned(),
        };
        assert_eq!(
            plan_startup(true, Some(&current), now, "0.1.0").notice,
            None
        );
    }

    #[test]
    fn a_disabled_check_never_touches_the_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = NetraConfig::with_default_data_dir();
        config.data_dir = dir.path().to_path_buf();
        config.update.check = false;
        startup_check(&config);
        assert!(!config.update_check_path().exists());
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "a disabled check writes nothing at all"
        );
    }

    #[test]
    fn the_cache_document_round_trips_through_a_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check.json");
        let cache = UpdateCache {
            checked_at: "2025-06-01T10:00:00+00:00".to_owned(),
            latest: "0.2.0".to_owned(),
        };
        write_cache(&path, &cache).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            r#"{"checked_at":"2025-06-01T10:00:00+00:00","latest":"0.2.0"}"#
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "the cache is private like the rest of the data dir"
            );
        }
        assert_eq!(read_cache(&path), Some(cache));
        assert!(
            std::fs::read_dir(dir.path()).unwrap().count() == 1,
            "the temporary file is renamed, not left behind"
        );
        std::fs::write(dir.path().join("corrupt.json"), "{ nope").unwrap();
        assert_eq!(read_cache(&dir.path().join("corrupt.json")), None);
    }

    #[test]
    fn installation_replaces_the_binary_atomically_in_a_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("netra");
        std::fs::write(&target, b"old build").unwrap();
        install_binary(&target, b"new build").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new build");
        assert!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_file()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "the replaced binary stays executable");
        }
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temporary and no .old file is left behind"
        );
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["netra".to_owned()]);
    }

    #[test]
    fn installation_refuses_a_target_that_is_not_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("netra");
        std::fs::create_dir(&target).unwrap();
        match install_binary(&target, b"new build") {
            Err(UpdateError::Install { path, reason }) => {
                assert_eq!(path, target);
                assert!(reason.contains("not a regular file"), "{reason}");
            }
            other => panic!("expected an installation error, got {other:?}"),
        }
        assert!(target.is_dir(), "the existing directory is untouched");

        let missing = dir.path().join("absent").join("netra");
        match install_binary(&missing, b"new build") {
            Err(UpdateError::Install { path, reason }) => {
                assert_eq!(path, missing);
                assert!(reason.contains("cannot write next to"), "{reason}");
            }
            other => panic!("expected an installation error, got {other:?}"),
        }
        assert!(!missing.exists());
    }

    #[test]
    fn the_install_error_points_at_the_escape_hatch() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("netra");
        std::fs::create_dir(&target).unwrap();
        let error = install_binary(&target, b"x").unwrap_err().to_string();
        assert!(error.contains(&target.display().to_string()), "{error}");
        assert!(error.contains(INSTALL_DIR_ENV), "{error}");
    }

    #[test]
    fn a_content_length_response_is_framed_without_waiting_for_eof() {
        let raw =
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\nhelloEXTRA";
        let response = parse_response("https://api.github.com/x", raw)
            .unwrap()
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"hello");
        assert_eq!(response.header("content-type"), Some("text/plain"));

        let truncated = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhel";
        assert_eq!(
            parse_response("https://api.github.com/x", truncated).unwrap(),
            None,
            "an incomplete body waits for more bytes"
        );
        let no_head = b"HTTP/1.1 200 OK\r\nContent-Length: 5";
        assert_eq!(
            parse_response("https://api.github.com/x", no_head).unwrap(),
            None
        );
        let unframed = b"HTTP/1.1 200 OK\r\nServer: x\r\n\r\nhello";
        assert!(
            parse_response("https://api.github.com/x", unframed).is_err(),
            "an unframed response would need EOF, which is refused"
        );
        let error_status = b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(
            parse_response("https://api.github.com/x", error_status)
                .unwrap()
                .unwrap()
                .status,
            404
        );
    }

    #[test]
    fn chunked_responses_are_decoded_and_truncated_ones_are_incomplete() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let response = parse_response("https://api.github.com/x", raw)
            .unwrap()
            .unwrap();
        assert_eq!(response.body, b"hello world");
        assert_eq!(response.header("transfer-encoding"), Some("chunked"));

        let truncated = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel";
        assert_eq!(
            parse_response("https://api.github.com/x", truncated).unwrap(),
            None
        );
        assert_eq!(decode_chunked(b"0\r\n\r\n").unwrap(), Some(Vec::new()));
        assert!(decode_chunked(b"zz\r\n").is_err());
    }

    #[test]
    fn a_404_answer_is_reported_as_no_release() {
        let url =
            Url::parse("https://api.github.com/repos/TzzJokerzzT/netra/releases/latest").unwrap();
        let not_found = HttpResponse {
            status: 404,
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert!(matches!(
            check_status(&url, &not_found),
            Err(UpdateError::NoRelease)
        ));
        let unknown = HttpResponse {
            status: 503,
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert!(matches!(
            check_status(&url, &unknown),
            Err(UpdateError::Response { .. })
        ));
        let ok = HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert!(check_status(&url, &ok).is_ok());
    }

    #[test]
    fn the_release_query_names_the_build_and_the_api_version() {
        let head = request_head(
            "api.github.com",
            443,
            "https",
            "/repos/TzzJokerzzT/netra/releases/latest",
            API_ACCEPT,
        );
        assert!(head.starts_with("GET /repos/TzzJokerzzT/netra/releases/latest HTTP/1.1\r\n"));
        assert!(head.contains(&format!("User-Agent: netra/{CURRENT_VERSION}\r\n")));
        assert!(head.contains("Accept: application/vnd.github+json\r\n"));
        assert!(head.contains("Host: api.github.com\r\n"));
        assert!(head.ends_with("\r\n\r\n"));
        let loopback = request_head("127.0.0.1", 8099, "http", "/download/netra", ASSET_ACCEPT);
        assert!(loopback.contains("Host: 127.0.0.1:8099\r\n"));
    }
}
