//! Target discovery over the browser's HTTP endpoint.
//!
//! Chrome publishes its protocol surface on a local HTTP port: `/json/version`
//! gives the browser WebSocket URL and `/json/list` lists targets. A minimal
//! HTTP client is used instead of pulling an HTTP stack into the capture crate;
//! the endpoint is loopback by design.

use std::fmt;

use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

/// A browser debugging endpoint (`host:port`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdpEndpoint {
    host: String,
    port: u16,
}

impl CdpEndpoint {
    /// Builds an endpoint from host and port.
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    /// Parses `host:port`, `http://host:port` or `ws://host:port`.
    ///
    /// # Errors
    ///
    /// Returns a description when the text has no usable host or port.
    pub fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        let without_scheme = trimmed
            .strip_prefix("http://")
            .or_else(|| trimmed.strip_prefix("ws://"))
            .or_else(|| trimmed.strip_prefix("https://"))
            .or_else(|| trimmed.strip_prefix("wss://"))
            .unwrap_or(trimmed);
        let authority = without_scheme.split('/').next().unwrap_or_default();
        match authority.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() => {
                let port: u16 = port
                    .parse()
                    .map_err(|_| format!("invalid CDP port in `{text}`"))?;
                Ok(Self::new(host, port))
            }
            _ => Err(format!(
                "invalid CDP endpoint `{text}`; expected host:port such as 127.0.0.1:9222"
            )),
        }
    }

    /// Host part.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Port part.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// `true` when the endpoint is a loopback address.
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        matches!(self.host.as_str(), "127.0.0.1" | "localhost" | "::1")
    }
}

impl fmt::Display for CdpEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// One target reported by the browser.
#[derive(Debug, Clone, Deserialize)]
pub struct TargetInfo {
    /// Target identifier.
    pub id: String,
    /// Target kind (`page`, `service_worker`, ...).
    #[serde(rename = "type")]
    pub kind: String,
    /// Current URL.
    #[serde(default)]
    pub url: String,
    /// Page title.
    #[serde(default)]
    pub title: String,
    /// Target-level WebSocket URL, when the browser exposes one.
    #[serde(rename = "webSocketDebuggerUrl", default)]
    pub websocket_url: Option<String>,
}

impl TargetInfo {
    /// `true` for targets whose network traffic the inspector follows.
    #[must_use]
    pub fn is_page(&self) -> bool {
        self.kind == "page"
    }
}

/// Discovery failures.
#[derive(Debug, thiserror::Error)]
pub enum CdpDiscoveryError {
    /// The endpoint refused the connection or timed out.
    #[error("cannot reach the Chrome debugging endpoint at {endpoint}: {reason}")]
    Unreachable {
        /// Endpoint that failed.
        endpoint: String,
        /// Underlying message.
        reason: String,
    },
    /// The endpoint answered with something that is not a CDP HTTP response.
    #[error("invalid response from {endpoint}: {reason}")]
    Invalid {
        /// Endpoint that answered.
        endpoint: String,
        /// What was wrong.
        reason: String,
    },
}

/// Fetches the browser-level WebSocket URL from `/json/version`.
///
/// # Errors
///
/// Returns [`CdpDiscoveryError`] when the endpoint is unreachable or the answer
/// carries no `webSocketDebuggerUrl`.
pub async fn browser_websocket_url(endpoint: &CdpEndpoint) -> Result<String, CdpDiscoveryError> {
    let body = http_get(endpoint, "/json/version").await?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| CdpDiscoveryError::Invalid {
            endpoint: endpoint.to_string(),
            reason: error.to_string(),
        })?;
    value
        .get("webSocketDebuggerUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| CdpDiscoveryError::Invalid {
            endpoint: endpoint.to_string(),
            reason: "the browser did not report a webSocketDebuggerUrl".to_owned(),
        })
}

/// Lists the browser's targets from `/json/list`.
///
/// # Errors
///
/// Returns [`CdpDiscoveryError`] when the endpoint is unreachable or the answer
/// is not a target list.
pub async fn discover_targets(
    endpoint: &CdpEndpoint,
) -> Result<Vec<TargetInfo>, CdpDiscoveryError> {
    let body = http_get(endpoint, "/json/list").await?;
    serde_json::from_str(&body).map_err(|error| CdpDiscoveryError::Invalid {
        endpoint: endpoint.to_string(),
        reason: error.to_string(),
    })
}

/// How long a discovery request may take.
const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Largest discovery response accepted, in bytes.
const MAX_DISCOVERY_BYTES: usize = 4 * 1024 * 1024;

/// Minimal HTTP/1.1 GET against the local debugging endpoint.
///
/// The browser keeps the connection alive, so the body is delimited by
/// `Content-Length` instead of by EOF.
async fn http_get(endpoint: &CdpEndpoint, path: &str) -> Result<String, CdpDiscoveryError> {
    match tokio::time::timeout(DISCOVERY_TIMEOUT, get_inner(endpoint, path)).await {
        Ok(result) => result,
        Err(_) => Err(CdpDiscoveryError::Unreachable {
            endpoint: endpoint.to_string(),
            reason: format!("no answer within {}s", DISCOVERY_TIMEOUT.as_secs()),
        }),
    }
}

async fn get_inner(endpoint: &CdpEndpoint, path: &str) -> Result<String, CdpDiscoveryError> {
    let address = format!("{}:{}", endpoint.host, endpoint.port);
    let mut stream =
        TcpStream::connect(&address)
            .await
            .map_err(|error| CdpDiscoveryError::Unreachable {
                endpoint: endpoint.to_string(),
                reason: error.to_string(),
            })?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| CdpDiscoveryError::Unreachable {
            endpoint: endpoint.to_string(),
            reason: error.to_string(),
        })?;
    let mut raw: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let read =
            stream
                .read(&mut chunk)
                .await
                .map_err(|error| CdpDiscoveryError::Unreachable {
                    endpoint: endpoint.to_string(),
                    reason: error.to_string(),
                })?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        if raw.len() > MAX_DISCOVERY_BYTES {
            return Err(CdpDiscoveryError::Invalid {
                endpoint: endpoint.to_string(),
                reason: "discovery response too large".to_owned(),
            });
        }
        if complete_body(&raw) {
            return finish(endpoint, &raw);
        }
    }
    finish(endpoint, &raw)
}

/// `true` once the headers and the declared body length have arrived.
fn complete_body(raw: &[u8]) -> bool {
    let Some(terminator) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let Ok(head) = std::str::from_utf8(&raw[..terminator]) else {
        return true;
    };
    let length: usize = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or_default();
    raw.len() >= terminator + 4 + length
}

fn finish(endpoint: &CdpEndpoint, raw: &[u8]) -> Result<String, CdpDiscoveryError> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| CdpDiscoveryError::Invalid {
            endpoint: endpoint.to_string(),
            reason: "response had no header terminator".to_owned(),
        })?;
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        let status = head.lines().next().unwrap_or("no status line").trim();
        return Err(CdpDiscoveryError::Invalid {
            endpoint: endpoint.to_string(),
            reason: format!("the browser answered: {status}"),
        });
    }
    Ok(body.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_parse_with_and_without_a_scheme() {
        assert_eq!(
            CdpEndpoint::parse("127.0.0.1:9222").unwrap(),
            CdpEndpoint::new("127.0.0.1", 9222)
        );
        assert_eq!(
            CdpEndpoint::parse("http://localhost:9333/").unwrap(),
            CdpEndpoint::new("localhost", 9333)
        );
        assert!(CdpEndpoint::parse("ws://127.0.0.1:9222/devtools/browser/x").is_ok());
        assert!(CdpEndpoint::parse("localhost").is_err());
        assert!(CdpEndpoint::parse("localhost:not-a-port").is_err());
        assert!(CdpEndpoint::parse(":9222").is_err());
    }

    #[test]
    fn loopback_detection_is_conservative() {
        assert!(CdpEndpoint::new("127.0.0.1", 9222).is_loopback());
        assert!(CdpEndpoint::new("::1", 9222).is_loopback());
        assert!(!CdpEndpoint::new("192.168.1.10", 9222).is_loopback());
    }

    #[test]
    fn target_list_parsing_tolerates_missing_fields() {
        let targets: Vec<TargetInfo> = serde_json::from_str(
            r#"[{"id":"A","type":"page","url":"https://a.test/","title":"A","webSocketDebuggerUrl":"ws://x"},
                {"id":"B","type":"service_worker"}]"#,
        )
        .unwrap();
        assert_eq!(targets.len(), 2);
        assert!(targets[0].is_page());
        assert!(targets[0].websocket_url.is_some());
        assert!(!targets[1].is_page());
        assert_eq!(targets[1].url, "");
    }
}
