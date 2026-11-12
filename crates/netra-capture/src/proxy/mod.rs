//! The HTTP/HTTPS proxy capture source (milestone M4).
//!
//! Scope, stated plainly:
//!
//! * **HTTP/1.1 forward proxying** with capture of request and response heads
//!   and bodies. One exchange per connection (`Connection: close`), which keeps
//!   framing unambiguous.
//! * **CONNECT tunnelling**: relayed byte-for-byte and *not* inspected. The
//!   exchange is reported as a tunnel so the inspector never pretends to know
//!   what travelled inside.
//! * **TLS interception** only when `proxy.intercept_tls` is enabled and the
//!   local CA exists: the proxy terminates TLS with a per-host certificate and
//!   captures the inner HTTP. Upstream TLS is verified with the Mozilla root
//!   store plus any configured extra roots; validation is never disabled.
//! * Loopback-only binding unless `proxy.allow_remote` is set, a connection
//!   ceiling, a body ceiling and per-step I/O timeouts.

pub mod ca;
pub mod message;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use netra_core::{
    config::ProxySettings,
    domain::{
        Body, CaptureEvent, CaptureSource, ContentType, HttpHeaders, HttpMethod, NetworkRequest,
        NetworkResponse, RequestUrl, SourceKey, StatusCode, Timestamp, TimingMetrics,
    },
    error::CaptureError,
    ports::CaptureProvider,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, copy_bidirectional},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc},
};
use tokio_rustls::{
    TlsAcceptor, TlsConnector,
    rustls::{
        ClientConfig, RootCertStore,
        pki_types::{CertificateDer, ServerName},
    },
};

use self::{
    ca::{CaPaths, CertificateAuthority},
    message::{
        BoxedStream, Connection, ProxyError, RequestHead, ResponseHead, parse_request_head,
        parse_response_head,
    },
};

/// Largest CONNECT authority accepted.
const MAX_AUTHORITY_BYTES: usize = 1024;

/// Proxy capture source.
pub struct ProxyCaptureProvider {
    settings: ProxySettings,
    data_dir: PathBuf,
    ca: Option<Arc<CertificateAuthority>>,
    running: bool,
    local_addr: Option<SocketAddr>,
    receiver: Option<mpsc::Receiver<CaptureEvent>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    sequence: Arc<AtomicU64>,
}

impl std::fmt::Debug for ProxyCaptureProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyCaptureProvider")
            .field("settings", &self.settings)
            .field("local_addr", &self.local_addr)
            .field("intercept_tls", &self.ca.is_some())
            .finish_non_exhaustive()
    }
}

impl ProxyCaptureProvider {
    /// Builds a provider from configuration.
    ///
    /// The CA is only loaded when interception is enabled, and loading it never
    /// creates it: `netra ca init` is the explicit action.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Unavailable`] when interception is enabled but
    /// the CA is missing or unreadable.
    pub fn new(settings: ProxySettings, data_dir: PathBuf) -> Result<Self, CaptureError> {
        let ca = if settings.intercept_tls {
            Some(Arc::new(
                CertificateAuthority::load(CaPaths::under(&data_dir)).map_err(|error| {
                    CaptureError::Unavailable(format!("TLS interception is enabled but {error}"))
                })?,
            ))
        } else {
            None
        };
        Ok(Self {
            settings,
            data_dir,
            ca,
            running: false,
            local_addr: None,
            receiver: None,
            tasks: Vec::new(),
            sequence: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Address the proxy actually listens on (useful when the port is `0`).
    #[must_use]
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    /// Data directory holding the CA.
    #[must_use]
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    fn upstream_client(&self) -> Result<Arc<ClientConfig>, CaptureError> {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for path in &self.settings.extra_trust_roots {
            let pem = std::fs::read(path).map_err(|error| {
                CaptureError::Source(format!(
                    "cannot read trust root {}: {error}",
                    path.display()
                ))
            })?;
            let mut reader = std::io::BufReader::new(pem.as_slice());
            for certificate in rustls_pemfile::certs(&mut reader) {
                let certificate: CertificateDer<'static> = certificate.map_err(|error| {
                    CaptureError::Source(format!("invalid trust root {}: {error}", path.display()))
                })?;
                roots.add(certificate).map_err(|error| {
                    CaptureError::Source(format!("unusable trust root {}: {error}", path.display()))
                })?;
            }
        }
        Ok(Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        ))
    }
}

#[async_trait]
impl CaptureProvider for ProxyCaptureProvider {
    fn source(&self) -> CaptureSource {
        CaptureSource::Proxy
    }

    async fn start(&mut self) -> Result<(), CaptureError> {
        if self.running {
            return Err(CaptureError::AlreadyRunning);
        }
        // Defence in depth: configuration validation already rejects this, but
        // the adapter must never open an open proxy on its own.
        let bind_host = self
            .settings
            .bind_addr
            .trim()
            .trim_matches(|c| c == '[' || c == ']');
        if !matches!(bind_host, "127.0.0.1" | "localhost" | "::1") && !self.settings.allow_remote {
            return Err(CaptureError::Unavailable(format!(
                "refusing to listen on {}: set proxy.allow_remote to expose the proxy beyond loopback",
                self.settings.bind_addr
            )));
        }
        let address = format!("{}:{}", self.settings.bind_addr.trim(), self.settings.port);
        let listener = TcpListener::bind(&address).await.map_err(|error| {
            CaptureError::Unavailable(format!("cannot listen on {address}: {error}"))
        })?;
        self.local_addr = listener.local_addr().ok();
        let semaphore = Arc::new(Semaphore::new(self.settings.max_connections));
        let (sender, receiver) = mpsc::channel(1024);
        let upstream = self.upstream_client()?;
        let acceptor = match self.ca.clone() {
            Some(ca) => Some(TlsAcceptor::from(Arc::new(
                ca.server_config()
                    .map_err(|error| CaptureError::Source(error.to_string()))?,
            ))),
            None => None,
        };
        let context = Arc::new(HandlerContext {
            settings: self.settings.clone(),
            upstream,
            acceptor,
            sender,
            sequence: Arc::clone(&self.sequence),
        });

        let listener_slot = listener;
        let accepting = tokio::spawn(async move {
            loop {
                let Ok(permit) = Arc::clone(&semaphore).acquire_owned().await else {
                    return;
                };
                match listener_slot.accept().await {
                    Ok((stream, _peer)) => {
                        let context = Arc::clone(&context);
                        tokio::spawn(async move {
                            if let Err(error) = handle_client(stream, context).await {
                                tracing::debug!(error = %error, "proxy connection ended");
                            }
                            drop(permit);
                        });
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "proxy accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        });
        self.running = true;
        self.receiver = Some(receiver);
        self.tasks.push(accepting);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CaptureError> {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self.running = false;
        self.receiver = None;
        Ok(())
    }

    async fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        let Some(receiver) = self.receiver.as_mut() else {
            return Err(CaptureError::NotRunning);
        };
        Ok(receiver.recv().await)
    }
}

/// Everything a connection handler needs.
struct HandlerContext {
    settings: ProxySettings,
    upstream: Arc<ClientConfig>,
    acceptor: Option<TlsAcceptor>,
    sender: mpsc::Sender<CaptureEvent>,
    sequence: Arc<AtomicU64>,
}

impl HandlerContext {
    fn timeout(&self) -> Duration {
        Duration::from_secs(self.settings.timeout_secs)
    }

    fn next_key(&self) -> SourceKey {
        SourceKey::new(
            CaptureSource::Proxy,
            format!("p{}", self.sequence.fetch_add(1, Ordering::Relaxed)),
        )
    }

    async fn emit(&self, event: CaptureEvent) {
        // A closed receiver means the runtime stopped; dropping is correct.
        let _ = self.sender.send(event).await;
    }
}

async fn handle_client(stream: TcpStream, context: Arc<HandlerContext>) -> Result<(), ProxyError> {
    stream.set_nodelay(true).ok();
    let mut connection = Connection::new(stream);
    let head = tokio::time::timeout(context.timeout(), connection.read_head())
        .await
        .map_err(|_| {
            ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "head timeout",
            ))
        })??;
    let Some(head) = head else {
        return Ok(());
    };
    let request = parse_request_head(&head)?;
    if request.is_connect() {
        return handle_connect(connection, request, context).await;
    }
    forward_plain(connection, request, context, "http").await
}

/// Handles `CONNECT host:port`: either a byte tunnel or TLS interception.
async fn handle_connect(
    connection: Connection<TcpStream>,
    request: RequestHead,
    context: Arc<HandlerContext>,
) -> Result<(), ProxyError> {
    let (host, port) = request.connect_authority()?;
    if host.len() > MAX_AUTHORITY_BYTES {
        return Err(ProxyError::Malformed(
            "CONNECT authority too long".to_owned(),
        ));
    }
    let authority = format!("{host}:{port}");
    let key = context.next_key();
    let started = Timestamp::now();
    let mut tunnel_request = NetworkRequest::new(
        HttpMethod::parse("CONNECT").map_err(|error| ProxyError::Malformed(error.to_string()))?,
        RequestUrl::new(format!("https://{authority}/")),
        started,
    );
    tunnel_request.headers = request.headers.clone();
    context
        .emit(CaptureEvent::RequestStarted {
            key: key.clone(),
            request: tunnel_request,
        })
        .await;

    match context.acceptor.clone() {
        None => tunnel(connection, &authority, key, context, started).await,
        Some(acceptor) => {
            let (stream, buffered) = connection.into_parts();
            if !buffered.is_empty() {
                return Err(ProxyError::Refused(
                    "the client sent TLS data before the tunnel was accepted".to_owned(),
                ));
            }
            context
                .emit(CaptureEvent::ResponseReceived {
                    key: key.clone(),
                    response: intercept_probe_response(),
                })
                .await;
            let mut plain = Connection::new(stream);
            plain
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await?;
            let tls = tokio::time::timeout(context.timeout(), acceptor.accept(stream_of(plain)))
                .await
                .map_err(|_| {
                    ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "TLS timeout",
                    ))
                })?
                .map_err(ProxyError::Io)?;
            forward_intercepted(tls, host, port, context).await
        }
    }
}

/// `acceptor.accept` needs the raw stream, not the buffered wrapper.
fn stream_of(connection: Connection<TcpStream>) -> TcpStream {
    connection.into_parts().0
}

/// Relays a `CONNECT` tunnel without inspecting it.
async fn tunnel(
    connection: Connection<TcpStream>,
    authority: &str,
    key: SourceKey,
    context: Arc<HandlerContext>,
    started: Timestamp,
) -> Result<(), ProxyError> {
    let upstream = tokio::time::timeout(context.timeout(), TcpStream::connect(authority))
        .await
        .map_err(|_| {
            ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connect timeout",
            ))
        })??;
    let (mut client, buffered) = connection.into_parts();
    if !buffered.is_empty() {
        client.write_all(&buffered).await?;
    }
    client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await?;
    let mut upstream = upstream;
    let relayed = copy_bidirectional(&mut client, &mut upstream).await;
    let total = started.elapsed_until(Timestamp::now());
    context
        .emit(CaptureEvent::RequestCompleted {
            key: key.clone(),
            timing: Some(TimingMetrics {
                total: Some(total),
                ..TimingMetrics::default()
            }),
            response: None,
        })
        .await;
    if let Err(error) = relayed {
        context
            .emit(CaptureEvent::RequestFailed {
                key,
                error: error.to_string(),
                timing: None,
            })
            .await;
    }
    Ok(())
}

/// Captures requests inside an intercepted TLS session.
async fn forward_intercepted(
    tls: tokio_rustls::server::TlsStream<TcpStream>,
    host: String,
    port: u16,
    context: Arc<HandlerContext>,
) -> Result<(), ProxyError> {
    let mut connection = Connection::new(tls);
    loop {
        let head = tokio::time::timeout(context.timeout(), connection.read_head())
            .await
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "head timeout",
                ))
            })??;
        let Some(head) = head else {
            return Ok(());
        };
        let request = parse_request_head(&head)?;
        let keep_alive = request.wants_keep_alive();
        let scheme = "https";
        let outcome = forward_one(
            &mut connection,
            request,
            &context,
            scheme,
            Some((host.clone(), port)),
        )
        .await?;
        if !keep_alive || !outcome {
            return Ok(());
        }
    }
}

/// Forwards one plain-HTTP request.
async fn forward_plain(
    mut connection: Connection<TcpStream>,
    request: RequestHead,
    context: Arc<HandlerContext>,
    scheme: &str,
) -> Result<(), ProxyError> {
    let keep_alive = request.wants_keep_alive();
    let outcome = forward_one(&mut connection, request, &context, scheme, None).await;
    let delivered = outcome?;
    if keep_alive && delivered {
        // One exchange per connection keeps framing unambiguous.
        return Ok(());
    }
    Ok(())
}

/// Executes one request/response exchange and captures both halves.
///
/// Returns whether the client should be considered in sync (used to decide
/// between closing and looping).
async fn forward_one<S>(
    connection: &mut Connection<S>,
    request: RequestHead,
    context: &Arc<HandlerContext>,
    scheme: &str,
    connect_override: Option<(String, u16)>,
) -> Result<bool, ProxyError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let started = Timestamp::now();
    let key = context.next_key();
    let limit = context.settings.max_body_bytes;
    let parsed = resolve_target(&request, scheme, connect_override)?;
    let request_body = tokio::time::timeout(
        context.timeout(),
        connection.read_body(&request.headers, limit),
    )
    .await
    .map_err(|_| {
        ProxyError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out waiting for the request body",
        ))
    })??;
    let mut network_request = NetworkRequest::new(
        HttpMethod::parse(&request.method)
            .map_err(|error| ProxyError::Malformed(error.to_string()))?,
        RequestUrl::new(parsed.url.clone()),
        started,
    );
    network_request.headers = request.headers.clone();
    network_request.body = match request_body.as_ref() {
        Some(bytes) => Body::captured(
            request.headers.get("content-type").map(ContentType::parse),
            bytes.clone(),
        ),
        None => absent_body(&request.headers),
    };
    context
        .emit(CaptureEvent::RequestStarted {
            key: key.clone(),
            request: network_request,
        })
        .await;

    let ResponseOutcome {
        head,
        body,
        body_bytes,
    } = fetch_upstream(&parsed, &request, request_body.as_deref(), context).await?;
    let received = Timestamp::now();
    let mut response = NetworkResponse::new(
        StatusCode::try_new(head.status).unwrap_or_else(|_| {
            StatusCode::try_new(502).unwrap_or_else(|_| unreachable!("502 is a valid status"))
        }),
        received,
    );
    response.status_text = Some(head.reason.clone());
    response.headers = head.headers.clone();
    let mut response_body = body;
    if let Some(bytes) = body_bytes.as_ref() {
        let content_type = head
            .headers
            .get("content-type")
            .map(ContentType::parse)
            .or_else(|| response_body.content_type.clone());
        response_body = Body::captured(content_type, bytes.clone())
            .with_size(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    }
    response.body = response_body;
    context
        .emit(CaptureEvent::ResponseReceived {
            key: key.clone(),
            response,
        })
        .await;

    let forwarded = forward_to_client(connection, &head, body_bytes.as_deref()).await;
    if forwarded.is_ok() {
        // Graceful half-close: a TLS client needs `close_notify`, and a plain
        // client needs the FIN to know the exchange is over.
        let _ = connection.shutdown().await;
    }
    let total = started.elapsed_until(Timestamp::now());
    context
        .emit(CaptureEvent::RequestCompleted {
            key: key.clone(),
            timing: Some(TimingMetrics {
                total: Some(total),
                ..TimingMetrics::default()
            }),
            response: None,
        })
        .await;
    forwarded?;
    Ok(true)
}

struct ResolvedTarget {
    url: String,
    host: String,
    port: u16,
    path: String,
    /// Upstream TLS is decided by the scheme, never by the port: an intercepted
    /// site on a non-standard port is still TLS.
    tls: bool,
}

fn resolve_target(
    request: &RequestHead,
    scheme: &str,
    connect_override: Option<(String, u16)>,
) -> Result<ResolvedTarget, ProxyError> {
    if let Some((host, port)) = connect_override {
        return Ok(ResolvedTarget {
            url: format!("{scheme}://{host}:{port}{}", request.target),
            host,
            port,
            path: request.target.clone(),
            tls: scheme == "https",
        });
    }
    let parsed = url::Url::parse(&request.target).map_err(|_| {
        ProxyError::Malformed(format!(
            "expected an absolute URI, received `{}`",
            request.target
        ))
    })?;
    let host = parsed
        .host_str()
        .ok_or_else(|| ProxyError::Malformed(format!("no host in {}", request.target)))?
        .to_owned();
    let port = parsed.port_or_known_default().unwrap_or(80);
    let path = match parsed.query() {
        Some(query) => format!("{}?{}", parsed.path(), query),
        None => parsed.path().to_owned(),
    };
    Ok(ResolvedTarget {
        url: parsed.to_string(),
        host,
        port,
        path,
        tls: parsed.scheme() == "https",
    })
}

struct ResponseOutcome {
    head: ResponseHead,
    body: Body,
    body_bytes: Option<Vec<u8>>,
}

async fn fetch_upstream(
    target: &ResolvedTarget,
    request: &RequestHead,
    body: Option<&[u8]>,
    context: &Arc<HandlerContext>,
) -> Result<ResponseOutcome, ProxyError> {
    let address = format!("{}:{}", target.host, target.port);
    let tcp = tokio::time::timeout(context.timeout(), TcpStream::connect(&address))
        .await
        .map_err(|_| {
            ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connect timeout",
            ))
        })??;
    tcp.set_nodelay(true).ok();
    let stream: BoxedStream = if target.tls {
        let connector = TlsConnector::from(Arc::clone(&context.upstream));
        let server_name = ServerName::try_from(target.host.clone())
            .map_err(|_| ProxyError::Malformed(format!("invalid TLS host: {}", target.host)))?;
        let tls = tokio::time::timeout(context.timeout(), connector.connect(server_name, tcp))
            .await
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "TLS timeout",
                ))
            })?
            .map_err(|error| ProxyError::Refused(format!("upstream TLS failed: {error}")))?;
        Box::new(tls)
    } else {
        Box::new(tcp)
    };
    let mut connection = Connection::new(stream);

    let mut head = format!("{} {} HTTP/1.1\r\n", request.method, target.path);
    for entry in request.headers.iter() {
        let name = entry.name.to_ascii_lowercase();
        if name.starts_with("proxy-") || name == "connection" || name == "transfer-encoding" {
            continue;
        }
        head.push_str(&entry.name);
        head.push_str(": ");
        head.push_str(&entry.value);
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n");
    if let Some(body) = body {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    connection.write_all(head.as_bytes()).await?;
    if let Some(body) = body {
        connection.write_all(body).await?;
    }
    let (head_text, body_bytes) = tokio::time::timeout(
        context.timeout(),
        read_response(&mut connection, context.settings.max_body_bytes),
    )
    .await
    .map_err(|_| {
        ProxyError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out waiting for the upstream response",
        ))
    })??;
    let head = parse_response_head(&head_text)?;
    let body = match body_bytes.as_ref() {
        Some(bytes) => Body::captured(
            head.headers.get("content-type").map(ContentType::parse),
            bytes.clone(),
        ),
        None => {
            let mut body = Body::absent();
            body.content_type = head.headers.get("content-type").map(ContentType::parse);
            body
        }
    };
    Ok(ResponseOutcome {
        head,
        body,
        body_bytes,
    })
}

async fn read_response<S: AsyncRead + AsyncWrite + Unpin>(
    connection: &mut Connection<S>,
    limit: usize,
) -> Result<(String, Option<Vec<u8>>), ProxyError> {
    let head = connection
        .read_head()
        .await?
        .ok_or_else(|| ProxyError::Malformed("upstream closed without a response".to_owned()))?;
    let parsed = parse_response_head(&head)?;
    let body = connection.read_body(&parsed.headers, limit).await?;
    Ok((head, body))
}

async fn forward_to_client<S: AsyncRead + AsyncWrite + Unpin>(
    connection: &mut Connection<S>,
    head: &ResponseHead,
    body: Option<&[u8]>,
) -> Result<(), ProxyError> {
    let mut forwarded = ResponseHead {
        version: "HTTP/1.1".to_owned(),
        status: head.status,
        reason: head.reason.clone(),
        headers: HttpHeaders::new(),
    };
    for entry in head.headers.iter() {
        let name = entry.name.to_ascii_lowercase();
        if name == "transfer-encoding" || name == "connection" || name == "content-length" {
            continue;
        }
        forwarded
            .headers
            .push(entry.name.clone(), entry.value.clone());
    }
    forwarded
        .headers
        .push("Content-Length", body.map_or(0, <[u8]>::len).to_string());
    forwarded.headers.push("Connection", "close");
    connection.write_all(&forwarded.to_bytes()).await?;
    if let Some(body) = body {
        connection.write_all(body).await?;
    }
    Ok(())
}

/// Synthetic response used to acknowledge a CONNECT before TLS starts.
fn intercept_probe_response() -> NetworkResponse {
    let mut response = NetworkResponse::new(
        StatusCode::try_new(200).unwrap_or_else(|_| unreachable!("200 is valid")),
        Timestamp::now(),
    );
    response.status_text = Some("Connection established".to_owned());
    response.body = Body::absent();
    response
}

fn absent_body(headers: &HttpHeaders) -> Body {
    let mut body = Body::absent();
    body.content_type = headers.get("content-type").map(ContentType::parse);
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_targets_are_resolved_with_default_ports() {
        let request = parse_request_head("GET http://example.com/a?b=1 HTTP/1.1").unwrap();
        let target = resolve_target(&request, "http", None).unwrap();
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 80);
        assert_eq!(target.path, "/a?b=1");
    }

    #[test]
    fn origin_form_targets_are_refused_outside_interception() {
        let request = parse_request_head("GET /relative HTTP/1.1").unwrap();
        assert!(resolve_target(&request, "http", None).is_err());
    }

    #[test]
    fn intercepted_targets_use_the_connect_authority() {
        let request = parse_request_head("GET /inner?q=1 HTTP/1.1").unwrap();
        let target = resolve_target(&request, "https", Some(("api.test".to_owned(), 443))).unwrap();
        assert_eq!(target.url, "https://api.test:443/inner?q=1");
        assert_eq!(target.port, 443);
    }

    #[test]
    fn client_headers_strip_hop_by_hop_fields() {
        let head = parse_response_head(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\nContent-Length: 99\r\nX-Keep: 1",
        )
        .unwrap();
        let mut forwarded = HttpHeaders::new();
        for entry in head.headers.iter() {
            let name = entry.name.to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "transfer-encoding" | "connection" | "content-length"
            ) {
                continue;
            }
            forwarded.push(entry.name.clone(), entry.value.clone());
        }
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded.get("x-keep"), Some("1"));
    }
}
