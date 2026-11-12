//! A scripted capture provider used to exercise the pipeline without a browser.
//!
//! This provider is *explicitly synthetic*: it is only constructed by the
//! `--demo` flag and its events carry the `DEMO` marker in fixed fixtures. It
//! must never be presented as real capture.

use async_trait::async_trait;
use netra_core::{
    domain::{
        Body, CaptureEvent, CaptureSource, ContentType, HttpHeaders, HttpMethod, NetworkRequest,
        NetworkResponse, RequestUrl, SourceKey, StatusCode, Timestamp, TimingMetrics,
    },
    error::CaptureError,
    ports::CaptureProvider,
};
use tokio::{
    sync::mpsc,
    time::{Duration, sleep},
};

/// Delay between scripted events.
const STEP_DELAY: Duration = Duration::from_millis(60);

/// Provider that replays a deterministic script of events.
#[derive(Debug)]
pub struct DemoCaptureProvider {
    source: CaptureSource,
    receiver: Option<mpsc::Receiver<CaptureEvent>>,
}

impl DemoCaptureProvider {
    /// Builds the demo provider. Events are produced lazily, starting with
    /// `start`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            source: CaptureSource::Cdp,
            receiver: None,
        }
    }
}

impl Default for DemoCaptureProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CaptureProvider for DemoCaptureProvider {
    fn source(&self) -> CaptureSource {
        self.source
    }

    async fn start(&mut self) -> Result<(), CaptureError> {
        if self.receiver.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        let (sender, receiver) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in script() {
                if sender.send(event).await.is_err() {
                    return;
                }
                sleep(STEP_DELAY).await;
            }
        });
        self.receiver = Some(receiver);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CaptureError> {
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

/// The fixed script: a GET with JSON, a pending POST, a redirect and a failure.
///
/// The method and status literals below are compile-time constants; a failed
/// parse would be a code defect, not an input error.
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn script() -> Vec<CaptureEvent> {
    let started = Timestamp::now();
    let mut events = Vec::new();

    let get_key = SourceKey::new(CaptureSource::Cdp, "demo-1");
    let mut get = NetworkRequest::new(
        HttpMethod::parse("GET").unwrap(),
        RequestUrl::new("https://api.demo.test/v1/users?page=1"),
        started,
    );
    get.headers = HttpHeaders::from_pairs([
        ("Accept", "application/json"),
        ("Authorization", "Bearer demo-token"),
    ]);
    events.push(CaptureEvent::RequestStarted {
        key: get_key.clone(),
        request: get,
    });

    let mut get_response = NetworkResponse::new(
        StatusCode::try_new(200).unwrap(),
        Timestamp::from_unix_millis(started.unix_millis() + 120),
    );
    get_response.headers = HttpHeaders::from_pairs([
        ("Content-Type", "application/json; charset=utf-8"),
        ("Cache-Control", "no-store"),
    ]);
    get_response.body = Body::captured(
        Some(ContentType::parse("application/json")),
        br#"{"users":[{"id":1,"name":"Ana"},{"id":2,"name":"Bruno"}]}"#.to_vec(),
    );
    events.push(CaptureEvent::ResponseReceived {
        key: get_key.clone(),
        response: get_response,
    });
    events.push(CaptureEvent::RequestCompleted {
        key: get_key,
        timing: Some(TimingMetrics {
            dns: Some(netra_core::domain::DurationMs::from_millis(4)),
            connect: Some(netra_core::domain::DurationMs::from_millis(12)),
            tls: Some(netra_core::domain::DurationMs::from_millis(18)),
            ttfb: Some(netra_core::domain::DurationMs::from_millis(74)),
            download: Some(netra_core::domain::DurationMs::from_millis(12)),
            total: Some(netra_core::domain::DurationMs::from_millis(120)),
            ..TimingMetrics::default()
        }),
        response: None,
    });

    let post_key = SourceKey::new(CaptureSource::Cdp, "demo-2");
    let mut post = NetworkRequest::new(
        HttpMethod::parse("POST").unwrap(),
        RequestUrl::new("https://api.demo.test/v1/orders"),
        Timestamp::from_unix_millis(started.unix_millis() + 200),
    );
    post.headers = HttpHeaders::from_pairs([
        ("Content-Type", "application/json"),
        ("Cookie", "session=demo"),
    ]);
    post.body = Body::captured(
        Some(ContentType::parse("application/json")),
        br#"{"item":"keyboard","password":"demo-secret"}"#.to_vec(),
    );
    events.push(CaptureEvent::RequestStarted {
        key: post_key.clone(),
        request: post,
    });

    let mut created = NetworkResponse::new(
        StatusCode::try_new(201).unwrap(),
        Timestamp::from_unix_millis(started.unix_millis() + 340),
    );
    created.headers = HttpHeaders::from_pairs([("Content-Type", "application/json")]);
    created.body = Body::captured(
        Some(ContentType::parse("application/json")),
        br#"{"id":"ord_1","status":"created"}"#.to_vec(),
    );
    events.push(CaptureEvent::ResponseReceived {
        key: post_key.clone(),
        response: created,
    });
    events.push(CaptureEvent::RequestCompleted {
        key: post_key,
        timing: Some(TimingMetrics {
            total: Some(netra_core::domain::DurationMs::from_millis(140)),
            ..TimingMetrics::default()
        }),
        response: None,
    });

    let redirect_key = SourceKey::new(CaptureSource::Proxy, "demo-3");
    events.push(CaptureEvent::RequestStarted {
        key: redirect_key.clone(),
        request: NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("http://demo.test/old"),
            Timestamp::from_unix_millis(started.unix_millis() + 500),
        ),
    });
    events.push(CaptureEvent::RequestRedirected {
        key: redirect_key,
        previous: NetworkResponse::new(
            StatusCode::try_new(301).unwrap(),
            Timestamp::from_unix_millis(started.unix_millis() + 520),
        ),
        request: NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("https://demo.test/new"),
            Timestamp::from_unix_millis(started.unix_millis() + 530),
        ),
    });

    events.push(CaptureEvent::RequestStarted {
        key: SourceKey::new(CaptureSource::Proxy, "demo-4"),
        request: NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("https://demo.test/broken"),
            Timestamp::from_unix_millis(started.unix_millis() + 700),
        ),
    });
    events.push(CaptureEvent::RequestFailed {
        key: SourceKey::new(CaptureSource::Proxy, "demo-4"),
        error: "net::ERR_CONNECTION_REFUSED".to_owned(),
        timing: None,
    });

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn script_is_deterministic_and_ends() {
        let mut provider = DemoCaptureProvider::new();
        provider.start().await.unwrap();
        let mut count = 0;
        while provider.next_event().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, script().len());
        assert!(provider.stop().await.is_ok());
    }

    #[tokio::test]
    async fn events_before_start_are_rejected() {
        let mut provider = DemoCaptureProvider::new();
        assert!(matches!(
            provider.next_event().await,
            Err(CaptureError::NotRunning)
        ));
    }
}
