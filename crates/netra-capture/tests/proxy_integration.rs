//! Proxy acceptance tests: capture over plain HTTP, CONNECT tunnelling, TLS
//! interception with the local authority, and the limits that keep the proxy
//! from becoming an open relay.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use netra_capture::proxy::{
    ProxyCaptureProvider,
    ca::{CaPaths, CertificateAuthority},
};
use netra_core::{
    config::ProxySettings,
    domain::{CaptureEvent, CaptureSource, ExchangeState},
    ports::CaptureProvider,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};

/// Local HTTP origin that answers `/api` with JSON and echoes other paths.
async fn spawn_http_origin() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 8192];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    return;
                }
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let body = if request.contains("/api") {
                    r#"{"service":"netra-proxy","ok":true}"#.to_owned()
                } else {
                    "echo".to_owned()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });
    address
}

fn settings(port: u16) -> ProxySettings {
    ProxySettings {
        bind_addr: "127.0.0.1".to_owned(),
        port,
        max_connections: 8,
        intercept_tls: false,
        max_body_bytes: 1024 * 1024,
        timeout_secs: 5,
        extra_trust_roots: Vec::new(),
        allow_remote: false,
    }
}

async fn collect_until_completed(provider: &mut ProxyCaptureProvider) -> Vec<CaptureEvent> {
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), provider.next_event()).await {
            Ok(Ok(Some(event))) => {
                let done = matches!(event, CaptureEvent::RequestCompleted { .. });
                events.push(event);
                if done {
                    return events;
                }
            }
            other => panic!("expected a completed exchange, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn plain_http_requests_are_captured_end_to_end() {
    let origin = spawn_http_origin().await;
    let data_dir = tempfile::tempdir().unwrap();
    let mut provider =
        ProxyCaptureProvider::new(settings(0), data_dir.path().to_path_buf()).unwrap();
    provider.start().await.unwrap();
    let proxy = provider.local_addr().unwrap();

    let mut client = TcpStream::connect(proxy).await.unwrap();
    let body = "{\"a\":\"bcd\"}\n";
    let request = format!(
        "POST http://{origin}/api/items?page=2 HTTP/1.1\r\nHost: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    client.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut raw)).await;
    let response = String::from_utf8_lossy(&raw);
    assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response:?}");
    assert!(response.contains("netra-proxy"), "{response}");

    let events = collect_until_completed(&mut provider).await;
    let started = events
        .iter()
        .find_map(|event| match event {
            CaptureEvent::RequestStarted { request, .. } => Some(request.clone()),
            _ => None,
        })
        .expect("a request start event");
    assert_eq!(started.method.as_str(), "POST");
    assert!(started.url.as_str().contains("/api/items?page=2"));
    assert_eq!(started.body.text().as_deref(), Some("{\"a\":\"bcd\"}\n"));
    let response = events
        .iter()
        .find_map(|event| match event {
            CaptureEvent::ResponseReceived { response, .. } => Some(response.clone()),
            _ => None,
        })
        .expect("a response event");
    assert_eq!(response.status.as_u16(), 200);
    assert!(response.body.text().unwrap().contains("netra-proxy"));
    assert!(matches!(
        events.last(),
        Some(CaptureEvent::RequestCompleted { .. })
    ));
    provider.stop().await.unwrap();
}

#[tokio::test]
async fn connect_tunnels_are_relayed_and_reported_as_tunnels() {
    let origin = spawn_http_origin().await;
    let data_dir = tempfile::tempdir().unwrap();
    let mut provider =
        ProxyCaptureProvider::new(settings(0), data_dir.path().to_path_buf()).unwrap();
    provider.start().await.unwrap();
    let proxy = provider.local_addr().unwrap();

    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(format!("CONNECT {origin} HTTP/1.1\r\nHost: {origin}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buffer = vec![0u8; 1024];
    let read = client.read(&mut buffer).await.unwrap();
    let established = String::from_utf8_lossy(&buffer[..read]).into_owned();
    assert!(established.starts_with("HTTP/1.1 200"), "{established}");

    // The tunnel is opaque to the proxy: raw bytes travel through it.
    client
        .write_all(b"GET /inside HTTP/1.1\r\nHost: origin\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut relayed = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut relayed)).await;
    client.shutdown().await.ok();
    assert!(
        String::from_utf8_lossy(&relayed).contains("echo"),
        "the tunnelled request must reach the origin"
    );

    let events = collect_until_completed(&mut provider).await;
    let started = events
        .iter()
        .find_map(|event| match event {
            CaptureEvent::RequestStarted { request, .. } => Some(request.clone()),
            _ => None,
        })
        .expect("a tunnel start event");
    assert_eq!(started.method.as_str(), "CONNECT");
    assert!(started.url.as_str().starts_with("https://127.0.0.1:"));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CaptureEvent::ResponseReceived { .. })),
        "a tunnel must not pretend to know the inner response"
    );
    provider.stop().await.unwrap();
}

#[tokio::test]
async fn oversized_requests_are_refused_without_capturing() {
    let origin = spawn_http_origin().await;
    let data_dir = tempfile::tempdir().unwrap();
    let mut provider = ProxyCaptureProvider::new(
        ProxySettings {
            max_body_bytes: 8,
            ..settings(0)
        },
        data_dir.path().to_path_buf(),
    )
    .unwrap();
    provider.start().await.unwrap();
    let proxy = provider.local_addr().unwrap();

    let mut client = TcpStream::connect(proxy).await.unwrap();
    let request = format!(
        "POST http://{origin}/api HTTP/1.1\r\nHost: {origin}\r\nContent-Length: 4096\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    let _ = client.read_to_end(&mut raw).await;
    assert!(
        raw.is_empty(),
        "the proxy must close instead of buffering an oversized body"
    );
    provider.stop().await.unwrap();
}

#[tokio::test]
async fn exposing_the_proxy_requires_an_explicit_opt_in() {
    let data_dir = tempfile::tempdir().unwrap();
    let mut provider = ProxyCaptureProvider::new(
        ProxySettings {
            bind_addr: "0.0.0.0".to_owned(),
            ..settings(0)
        },
        data_dir.path().to_path_buf(),
    )
    .unwrap();
    let error = provider.start().await.expect_err("must refuse");
    assert!(error.to_string().contains("allow_remote"));
}

#[tokio::test]
async fn interception_without_an_authority_fails_before_listening() {
    let data_dir = tempfile::tempdir().unwrap();
    let error = ProxyCaptureProvider::new(
        ProxySettings {
            intercept_tls: true,
            ..settings(0)
        },
        data_dir.path().to_path_buf(),
    )
    .expect_err("interception needs a CA");
    assert!(error.to_string().contains("netra ca init"));
}

#[tokio::test]
async fn tls_interception_captures_the_decrypted_exchange() {
    let data_dir = tempfile::tempdir().unwrap();
    // The inspector's authority, created explicitly like `netra ca init` does.
    let authority =
        Arc::new(CertificateAuthority::load_or_create(CaPaths::under(data_dir.path())).unwrap());
    let (origin_address, origin_ca) = spawn_tls_origin().await;
    let origin_ca_path = data_dir.path().join("origin-ca.pem");
    std::fs::write(&origin_ca_path, origin_ca).unwrap();

    let mut provider = ProxyCaptureProvider::new(
        ProxySettings {
            intercept_tls: true,
            extra_trust_roots: vec![origin_ca_path],
            ..settings(0)
        },
        data_dir.path().to_path_buf(),
    )
    .unwrap();
    provider.start().await.unwrap();
    let proxy = provider.local_addr().unwrap();

    // A client that trusts the inspector's CA, as a configured browser would.
    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(
            format!("CONNECT {origin_address} HTTP/1.1\r\nHost: {origin_address}\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut buffer = vec![0u8; 256];
    let read = client.read(&mut buffer).await.unwrap();
    assert!(String::from_utf8_lossy(&buffer[..read]).starts_with("HTTP/1.1 200"));

    let client_config = client_config_trusting(authority.certificate_pem());
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
    let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls = connector.connect(server_name, client).await.unwrap();
    tls.write_all(
        b"GET /inner?token=secret HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut raw = Vec::new();
    let mut read_error = None;
    loop {
        let mut chunk = [0u8; 1024];
        match tls.read(&mut chunk).await {
            Ok(0) => break,
            Ok(count) => raw.extend_from_slice(&chunk[..count]),
            Err(error) => {
                read_error = Some(error.to_string());
                break;
            }
        }
    }
    let response = String::from_utf8_lossy(&raw).into_owned();
    assert!(
        response.contains("secure-body"),
        "got {response:?} (error: {read_error:?})"
    );

    let mut events = Vec::new();
    while let Ok(Ok(Some(event))) =
        tokio::time::timeout(Duration::from_secs(5), provider.next_event()).await
    {
        let done = matches!(event, CaptureEvent::RequestCompleted { .. });
        events.push(event);
        if done {
            break;
        }
    }
    let inner = events
        .iter()
        .rev()
        .find_map(|event| match event {
            CaptureEvent::RequestStarted { request, .. } => Some(request.clone()),
            _ => None,
        })
        .expect("the decrypted request must be captured");
    assert!(
        inner.url.as_str().starts_with("https://"),
        "{:?}",
        inner.url
    );
    assert!(inner.url.as_str().contains("/inner?token=secret"));
    let response = events
        .iter()
        .rev()
        .find_map(|event| match event {
            CaptureEvent::ResponseReceived { response, .. } => Some(response.clone()),
            _ => None,
        })
        .expect("the decrypted response must be captured");
    assert_eq!(response.status.as_u16(), 200);
    assert!(response.body.text().unwrap().contains("secure-body"));
    assert_eq!(ExchangeState::Completed.as_str(), "completed");
    assert_eq!(provider.source(), CaptureSource::Proxy);
    provider.stop().await.unwrap();
}

fn client_config_trusting(ca_pem: &str) -> tokio_rustls::rustls::ClientConfig {
    use tokio_rustls::rustls::{ClientConfig, RootCertStore, pki_types::CertificateDer};
    let mut roots = RootCertStore::empty();
    let der = pem_to_der(ca_pem).expect("valid CA pem");
    roots.add(CertificateDer::from(der)).unwrap();
    ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

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

/// Local TLS origin signed by its own throwaway authority.
async fn spawn_tls_origin() -> (SocketAddr, String) {
    use rcgen::{CertificateParams, DnType, KeyPair};
    use tokio_rustls::rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    };

    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::new()).unwrap();
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "netra test origin CA");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca_params, ca_key);

    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf_params =
        CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()]).unwrap();
    leaf_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    let leaf = leaf_params.signed_by(&leaf_key, &issuer).unwrap();

    let chain = vec![
        CertificateDer::from(leaf.der().to_vec()),
        CertificateDer::from(ca_cert.der().to_vec()),
    ];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut buffer = vec![0u8; 4096];
                let _ = tls.read(&mut buffer).await;
                let body = r#"{"secure":"secure-body"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.flush().await;
            });
        }
    });
    (address, ca_cert.pem())
}

/// Keeps `PathBuf` imported for the settings helper in future tests.
const _: fn() -> PathBuf = PathBuf::new;
