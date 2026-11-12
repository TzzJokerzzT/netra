//! Local certificate authority for TLS interception.
//!
//! Generating a CA is an explicit user action (`netra ca init`); nothing is
//! installed into any trust store. The private key is written `0600` and a leaf
//! certificate is minted per host on demand, only while interception is enabled.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{
    ServerConfig,
    crypto::ring,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};

/// Common name of the generated authority.
pub const CA_COMMON_NAME: &str = "netra local CA";
/// Organization recorded in the authority subject.
pub const CA_ORGANIZATION: &str = "netra";

/// Failures while creating, loading or using the authority.
#[derive(Debug, thiserror::Error)]
pub enum CaError {
    /// Filesystem problem.
    #[error("cannot access {path}: {reason}")]
    Io {
        /// Offending path.
        path: PathBuf,
        /// Underlying message.
        reason: String,
    },
    /// The material exists but cannot be used.
    #[error("invalid certificate material: {0}")]
    Invalid(String),
    /// The authority has not been created yet.
    #[error("no local CA yet at {0}; run `netra ca init` first")]
    Missing(PathBuf),
}

/// Files the authority lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaPaths {
    /// Directory holding the material.
    pub directory: PathBuf,
    /// PEM certificate handed to the user for trust.
    pub certificate: PathBuf,
    /// PEM private key.
    pub key: PathBuf,
}

impl CaPaths {
    /// Derives the three paths from the data directory.
    #[must_use]
    pub fn under(data_dir: &Path) -> Self {
        let directory = data_dir.join("ca");
        Self {
            certificate: directory.join("netra-ca.pem"),
            key: directory.join("netra-ca.key"),
            directory,
        }
    }
}

/// The authority in memory: certificate PEM plus signing key.
pub struct CertificateAuthority {
    certificate_pem: String,
    /// Rebuilt to sign leaves; matches the persisted certificate subject.
    issuer: Issuer<'static, KeyPair>,
    paths: CaPaths,
}

impl std::fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateAuthority")
            .field("certificate", &self.paths.certificate)
            .finish_non_exhaustive()
    }
}

impl CertificateAuthority {
    /// Creates the authority if it does not exist yet, and loads it otherwise.
    ///
    /// This is the only place that writes key material, and it never overwrites
    /// an existing authority: replacing a CA silently would break every trust
    /// decision the user already made.
    ///
    /// # Errors
    ///
    /// Returns [`CaError`] when the directory cannot be created or the existing
    /// files cannot be read.
    pub fn load_or_create(paths: CaPaths) -> Result<Self, CaError> {
        if paths.certificate.exists() && paths.key.exists() {
            return Self::load(paths);
        }
        create_private_directory(&paths.directory)?;
        let key = KeyPair::generate().map_err(|error| CaError::Invalid(error.to_string()))?;
        let params = ca_params()?;
        let certificate = params
            .self_signed(&key)
            .map_err(|error| CaError::Invalid(error.to_string()))?;
        write_private(&paths.certificate, certificate.pem().as_bytes())?;
        write_private(&paths.key, key.serialize_pem().as_bytes())?;
        Ok(Self {
            certificate_pem: certificate.pem(),
            issuer: Issuer::new(params, key),
            paths,
        })
    }

    /// Loads an existing authority.
    ///
    /// # Errors
    ///
    /// Returns [`CaError::Missing`] when the material does not exist and
    /// [`CaError::Invalid`] when the key cannot be parsed.
    pub fn load(paths: CaPaths) -> Result<Self, CaError> {
        if !paths.certificate.exists() || !paths.key.exists() {
            return Err(CaError::Missing(paths.certificate));
        }
        let certificate_pem = read_to_string(&paths.certificate)?;
        let key_pem = read_to_string(&paths.key)?;
        let key = KeyPair::from_pem(&key_pem)
            .map_err(|error| CaError::Invalid(format!("private key: {error}")))?;
        let params = ca_params()?;
        Ok(Self {
            certificate_pem,
            issuer: Issuer::new(params, key),
            paths,
        })
    }

    /// Paths of the material.
    #[must_use]
    pub fn paths(&self) -> &CaPaths {
        &self.paths
    }

    /// PEM certificate to hand to a browser or the OS trust store.
    #[must_use]
    pub fn certificate_pem(&self) -> &str {
        &self.certificate_pem
    }

    /// Mints a leaf certificate for one host.
    ///
    /// # Errors
    ///
    /// Returns [`CaError::Invalid`] when the host cannot be encoded or the
    /// signature fails.
    pub fn leaf_for(&self, host: &str) -> Result<Arc<CertifiedKey>, CaError> {
        let mut params = CertificateParams::new(vec![host.to_owned()])
            .map_err(|error| CaError::Invalid(error.to_string()))?;
        params
            .distinguished_name
            .push(DnType::CommonName, host.to_owned());
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2039, 1, 1);
        let key = KeyPair::generate().map_err(|error| CaError::Invalid(error.to_string()))?;
        let certificate = params
            .signed_by(&key, &self.issuer)
            .map_err(|error| CaError::Invalid(error.to_string()))?;
        let chain = vec![
            CertificateDer::from(certificate.der().to_vec()),
            CertificateDer::from(pem_to_der(&self.certificate_pem).unwrap_or_default()),
        ];
        let key_der = PrivatePkcs8KeyDer::from(key.serialize_der());
        let signing_key = ring::sign::any_supported_type(&PrivateKeyDer::Pkcs8(key_der))
            .map_err(|error| CaError::Invalid(error.to_string()))?;
        Ok(Arc::new(CertifiedKey::new(chain, signing_key)))
    }

    /// Builds a rustls server configuration that mints a certificate per host.
    ///
    /// # Errors
    ///
    /// Returns [`CaError::Invalid`] when the CA cannot sign.
    pub fn server_config(self: Arc<Self>) -> Result<ServerConfig, CaError> {
        // Fail now rather than mid-handshake if the authority is unusable.
        let probe = self.leaf_for("probe.invalid")?;
        drop(probe);
        let resolver = Arc::new(PerHostResolver {
            authority: Arc::clone(&self),
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        });
        Ok(ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver))
    }
}

/// Resolves a certificate per requested host, minting and caching on demand.
struct PerHostResolver {
    authority: Arc<CertificateAuthority>,
    cache: std::sync::Mutex<std::collections::HashMap<String, Arc<CertifiedKey>>>,
}

impl std::fmt::Debug for PerHostResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PerHostResolver")
    }
}

impl ResolvesServerCert for PerHostResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let host = hello.server_name()?.to_ascii_lowercase();
        if let Ok(cache) = self.cache.lock()
            && let Some(certified) = cache.get(&host)
        {
            return Some(Arc::clone(certified));
        }
        let certified = self.authority.leaf_for(&host).ok()?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(host, Arc::clone(&certified));
        }
        Some(certified)
    }
}

fn ca_params() -> Result<CertificateParams, CaError> {
    let mut params =
        CertificateParams::new(Vec::new()).map_err(|error| CaError::Invalid(error.to_string()))?;
    params
        .distinguished_name
        .push(DnType::CommonName, CA_COMMON_NAME);
    params
        .distinguished_name
        .push(DnType::OrganizationName, CA_ORGANIZATION);
    // Explicit validity keeps the authority usable across clock skew and does
    // not pretend to be valid for millennia.
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after = rcgen::date_time_ymd(2039, 1, 1);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    Ok(params)
}

fn create_private_directory(path: &Path) -> Result<(), CaError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(|error| CaError::Io {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path).map_err(|error| CaError::Io {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })
    }
}

fn write_private(path: &Path, contents: &[u8]) -> Result<(), CaError> {
    use std::io::Write as _;
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| CaError::Io {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?
    };
    #[cfg(not(unix))]
    let mut file = std::fs::File::create(path).map_err(|error| CaError::Io {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    file.write_all(contents).map_err(|error| CaError::Io {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

fn read_to_string(path: &Path) -> Result<String, CaError> {
    std::fs::read_to_string(path).map_err(|error| CaError::Io {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Extracts DER bytes from a single PEM certificate block.
fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority() -> (tempfile::TempDir, CertificateAuthority) {
        let dir = tempfile::tempdir().unwrap();
        let paths = CaPaths::under(dir.path());
        let ca = CertificateAuthority::load_or_create(paths).unwrap();
        (dir, ca)
    }

    #[test]
    fn creates_private_material_once() {
        let dir = tempfile::tempdir().unwrap();
        let paths = CaPaths::under(dir.path());
        let ca = CertificateAuthority::load_or_create(paths.clone()).unwrap();
        assert!(ca.certificate_pem().contains("BEGIN CERTIFICATE"));
        let reopened = CertificateAuthority::load_or_create(paths.clone()).unwrap();
        assert_eq!(
            reopened.certificate_pem(),
            ca.certificate_pem(),
            "reopening must not replace the authority"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&paths.key).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the private key must be private");
            let certificate = std::fs::metadata(&paths.certificate)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(certificate, 0o600);
        }
    }

    #[test]
    fn loading_missing_material_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let error = CertificateAuthority::load(CaPaths::under(dir.path())).unwrap_err();
        assert!(matches!(error, CaError::Missing(_)));
        assert!(error.to_string().contains("netra ca init"));
    }

    #[test]
    fn leaves_are_minted_per_host_and_chained_to_the_authority() {
        let (_dir, ca) = authority();
        let leaf = ca.leaf_for("api.example.com").unwrap();
        assert_eq!(leaf.cert.len(), 2, "leaf plus issuer chain");
        let other = ca.leaf_for("api.example.com").unwrap();
        assert_ne!(
            leaf.cert[0], other.cert[0],
            "each call must mint a fresh leaf, never reuse a key"
        );
        assert_eq!(
            leaf.cert[1], other.cert[1],
            "both leaves chain to the same authority"
        );
        let ca_der = pem_to_der(ca.certificate_pem()).unwrap();
        assert_eq!(leaf.cert[1].as_ref(), ca_der.as_slice());
    }

    #[test]
    fn server_config_is_built_from_the_authority() {
        let (_dir, ca) = authority();
        let config = Arc::new(ca).server_config().unwrap();
        assert!(config.alpn_protocols.is_empty());
    }
}
