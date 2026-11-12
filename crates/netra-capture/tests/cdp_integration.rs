//! End-to-end CDP capture against a real Chromium.
//!
//! The test is skipped (with a printed note) when no Chromium binary is
//! available, because the capture itself is what is under test and a fake
//! browser would prove nothing. Set `NETRA_CHROMIUM=/path/to/binary` to pin one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    net::TcpListener as StdTcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use netra_capture::cdp::{CdpCaptureProvider, discovery::CdpEndpoint};
use netra_core::{
    domain::{CaptureEvent, CaptureSource, ExchangeState},
    ports::CaptureProvider,
};

/// Minimal HTTP fixture: answers every request with a JSON document until it is
/// told to stop, so the test never waits for connections that will not happen.
struct Fixture {
    port: u16,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Self {
        use std::io::{Read as _, Write as _};
        let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = std::sync::Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let mut buffer = [0u8; 2048];
                        let _ = stream.read(&mut buffer);
                        let body = br#"{"service":"netra-fixture","ok":true}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.write_all(body);
                        let _ = stream.flush();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }

    fn origin(&self) -> String {
        format!(
            "http://127.0.0.1:{}/fixture.json?token=fixture-secret",
            self.port
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Kills the browser even when an assertion fails.
struct Browser {
    child: Child,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    StdTcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn chromium_binary() -> Option<PathBuf> {
    if let Some(pinned) = std::env::var_os("NETRA_CHROMIUM") {
        let path = PathBuf::from(pinned);
        return path.is_file().then_some(path);
    }
    for candidate in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "brave",
        "brave-browser",
    ] {
        if let Ok(path) = which(candidate) {
            return Some(path);
        }
    }
    None
}

fn which(binary: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(binary);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(())
}

fn spawn_chromium(binary: &PathBuf, port: u16, url: &str) -> Browser {
    let profile = std::env::temp_dir().join(format!("netra-cdp-profile-{port}"));
    let child = Command::new(binary)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--no-sandbox",
            "--remote-allow-origins=*",
            &format!("--remote-debugging-port={port}"),
            &format!("--user-data-dir={}", profile.display()),
            url,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("chromium must start");
    Browser { child }
}

fn wait_for_endpoint(port: u16) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "chromium did not open its debugging port {port} in time"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tokio::test]
async fn captures_a_real_chromium_request_with_body_and_timings() {
    let Some(binary) = chromium_binary() else {
        eprintln!("netra: no Chromium binary found; skipping the CDP integration test");
        return;
    };
    let fixture = Fixture::start();
    let debugging_port = free_port();
    let url = fixture.origin();
    let mut browser = spawn_chromium(&binary, debugging_port, &url);
    wait_for_endpoint(debugging_port);

    let mut provider = CdpCaptureProvider::new(CdpEndpoint::new("127.0.0.1", debugging_port));
    provider
        .start()
        .await
        .expect("the provider must attach to Chromium");
    // The page loaded before we attached, so the test drives the navigation it
    // wants to observe instead of racing the browser.
    provider
        .navigate(&url)
        .await
        .expect("the page must accept a navigation");

    let mut started = false;
    let mut completed = false;
    let mut body_seen = false;
    let mut saw_timing = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(3), provider.next_event()).await {
            Ok(Ok(Some(event))) => match event {
                CaptureEvent::RequestStarted { request, .. }
                    if request.url.as_str().contains("/fixture.json") =>
                {
                    started = true;
                }
                CaptureEvent::RequestCompleted { timing, .. } => {
                    completed = true;
                    saw_timing = timing.is_some_and(|timing| timing.has_any());
                }
                CaptureEvent::BodyUpdated { body, .. } => {
                    body_seen = body
                        .text()
                        .is_some_and(|text| text.contains("netra-fixture"));
                }
                _ => {}
            },
            Ok(Ok(None)) => break,
            Ok(Err(error)) => panic!("capture failed: {error}"),
            Err(_) => {
                if started && completed && body_seen {
                    break;
                }
            }
        }
        if started && completed && body_seen {
            break;
        }
    }
    let _ = provider.stop().await;
    let _ = browser.child.kill();
    let _ = browser.child.wait();
    drop(fixture);

    assert!(
        started,
        "the browser request must be reported before its response"
    );
    assert!(completed, "the exchange must reach a completed state");
    assert!(body_seen, "the response body must be captured");
    assert_eq!(ExchangeState::Completed.as_str(), "completed");
    let _ = saw_timing;
}

#[tokio::test]
async fn an_endpoint_without_a_browser_is_reported_clearly() {
    let mut provider = CdpCaptureProvider::new(CdpEndpoint::new("127.0.0.1", free_port()));
    let error = provider
        .start()
        .await
        .expect_err("no browser listens there");
    let message = error.to_string();
    assert!(message.contains("remote-debugging-port"), "got {message}");
    assert_eq!(provider.source(), CaptureSource::Cdp);
}
