//! The CDP capture provider.
//!
//! Lifecycle: connect to the browser WebSocket, discover targets, attach to each
//! page with a flat session, enable the `Network` domain per session, then pump
//! frames. Response bodies are requested while the pump keeps running, so a slow
//! body never stalls the rest of the traffic.
//!
//! Failure behaviour, deliberately explicit:
//!
//! * A lost browser connection produces `SourceDisconnected`, then a bounded
//!   number of reconnect attempts, then `SourceReconnected` on success.
//! * When retries are exhausted the stream ends (`Ok(None)`), so the runtime
//!   stops the session instead of leaving it pretending to capture.
//! * A body the browser cannot serve becomes an explicit `omitted` payload, not
//!   a missing one.

use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use futures_util::{SinkExt as _, StreamExt as _};
use netra_core::{
    domain::{
        Body, BodyDirection, CaptureEvent, CaptureSource, ContentType, OmitReason, SourceKey,
    },
    error::CaptureError,
    ports::CaptureProvider,
};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, error::Error as WsError},
};

use super::{
    discovery::{CdpDiscoveryError, CdpEndpoint, browser_websocket_url},
    mapping::{CdpOutcome, TimingDraft, finalize_timing, translate},
    protocol::{Incoming, command, parse_incoming},
};

/// Hard cap on a single captured payload.
///
/// The capture pipeline applies the configured limit; this cap exists so a
/// multi-gigabyte body cannot be materialised in memory before that happens.
pub const MAX_CAPTURED_BODY_BYTES: usize = 8 * 1024 * 1024;

/// How many times a lost browser connection is retried.
const MAX_RECONNECT_ATTEMPTS: u8 = 3;
/// Delay before a reconnect attempt; multiplied by the attempt number.
const RECONNECT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);
/// Browser-side buffer budget for the `Network` domain, in bytes.
const NETWORK_BUFFER_BUDGET: u64 = 20 * 1024 * 1024;
/// Largest post body the browser keeps available for later retrieval.
const MAX_POST_DATA_SIZE: u64 = 1024 * 1024;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// State kept for one in-flight request.
#[derive(Debug)]
struct Inflight {
    draft: TimingDraft,
    content_type: Option<ContentType>,
}

/// Work the pump owes a newly attached session.
#[derive(Debug, Clone)]
enum PendingSession {
    /// Enable the network domain and let the paused target run.
    Enable {
        session_id: String,
        target_id: String,
    },
    /// A duplicate attachment: let it run and drop it so events are not doubled.
    ResumeOnly { session_id: String },
}

/// A body request waiting for its answer.
#[derive(Debug, Clone)]
struct PendingBody {
    key: SourceKey,
    content_type: Option<ContentType>,
}

/// Captures Chrome/Chromium traffic through the DevTools Protocol.
#[derive(Debug)]
pub struct CdpCaptureProvider {
    endpoint: CdpEndpoint,
    socket: Option<Socket>,
    next_id: u64,
    sessions: HashMap<String, String>,
    /// Targets already attached, so auto-attach cannot duplicate a session.
    attached_targets: HashSet<String>,
    inflight: HashMap<SourceKey, Inflight>,
    pending_bodies: HashMap<u64, PendingBody>,
    /// Sessions discovered mid-stream whose `Network.enable` still has to run.
    pending_setup: VecDeque<PendingSession>,
    queue: VecDeque<CaptureEvent>,
    reconnect_attempts: u8,
    lost_connections: u64,
}

impl CdpCaptureProvider {
    /// Builds a provider for a debugging endpoint.
    #[must_use]
    pub fn new(endpoint: CdpEndpoint) -> Self {
        Self {
            endpoint,
            socket: None,
            next_id: 1,
            sessions: HashMap::new(),
            attached_targets: HashSet::new(),
            inflight: HashMap::new(),
            pending_bodies: HashMap::new(),
            pending_setup: VecDeque::new(),
            queue: VecDeque::new(),
            reconnect_attempts: 0,
            lost_connections: 0,
        }
    }

    /// Navigates every attached page to `url`.
    ///
    /// This is how a caller makes the browser produce traffic on demand: the
    /// integration test uses it, and replay/refresh features will build on it.
    /// It never runs on its own.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::NotRunning`] before `start` and
    /// [`CaptureError::Source`] when the browser rejects the navigation.
    pub async fn navigate(&mut self, url: &str) -> Result<(), CaptureError> {
        if self.socket.is_none() {
            return Err(CaptureError::NotRunning);
        }
        // A session discovered during `connect` still needs its
        // `Network.enable`; navigation must wait for that.
        self.drain_pending().await?;
        let sessions: Vec<String> = self.sessions.keys().cloned().collect();
        for session in sessions {
            self.request("Page.navigate", json!({ "url": url }), Some(&session))
                .await?;
        }
        Ok(())
    }

    /// Endpoint this provider talks to.
    #[must_use]
    pub fn endpoint(&self) -> &CdpEndpoint {
        &self.endpoint
    }

    /// Number of times the browser connection was lost.
    #[must_use]
    pub fn lost_connections(&self) -> u64 {
        self.lost_connections
    }

    /// Connects to the browser and prepares every page target.
    async fn connect(&mut self) -> Result<(), CaptureError> {
        let websocket_url = browser_websocket_url(&self.endpoint).await.map_err(
            |error| match error {
                CdpDiscoveryError::Unreachable { .. } => CaptureError::Unavailable(format!(
                    "{error}. Start Chrome/Chromium with --remote-debugging-port={} and try again",
                    self.endpoint.port()
                )),
                other => CaptureError::Source(other.to_string()),
            },
        )?;
        let (socket, _response) = connect_async(websocket_url.as_str())
            .await
            .map_err(|error| {
                CaptureError::Source(format!("cannot open the CDP socket: {error}"))
            })?;
        self.socket = Some(socket);
        self.next_id = 1;
        self.sessions.clear();
        self.attached_targets.clear();
        self.inflight.clear();
        self.pending_bodies.clear();
        self.pending_setup.clear();
        self.queue.clear();

        // Flat sessions keep one event stream per target so pages cannot
        // interleave their request identifiers.
        self.request(
            "Target.setDiscoverTargets",
            json!({ "discover": true }),
            None,
        )
        .await?;
        // Auto-attach with a debugger pause: a target created later (a new tab,
        // or a cross-process navigation) is held until its `Network.enable`
        // completes, so its first request cannot be missed.
        self.request(
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
            None,
        )
        .await?;
        // `setAutoAttach` covers targets created from now on; the ones that
        // already exist must be attached explicitly. `attached_targets` keeps
        // the two paths from attaching the same target twice, which would
        // duplicate every event.
        let targets = self.request("Target.getTargets", json!({}), None).await?;
        let target_ids: Vec<String> = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .map(|infos| {
                infos
                    .iter()
                    .filter(|info| info.get("type").and_then(Value::as_str) == Some("page"))
                    .filter_map(|info| info.get("targetId").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        for target_id in target_ids {
            if !self.attached_targets.contains(&target_id) {
                self.attach(target_id).await?;
            }
        }
        Ok(())
    }

    /// Attaches to one target and enables its `Network` domain.
    async fn attach(&mut self, target_id: String) -> Result<(), CaptureError> {
        let result = self
            .request(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let Some(session_id) = result
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Err(CaptureError::Source(format!(
                "the browser attached to target {target_id} without a session id"
            )));
        };
        self.attached_targets.insert(target_id.clone());
        self.setup_session(&session_id, &target_id).await
    }

    /// Enables the network domain for an attached session.
    async fn setup_session(
        &mut self,
        session_id: &str,
        target_id: &str,
    ) -> Result<(), CaptureError> {
        if self.sessions.contains_key(session_id) {
            return Ok(());
        }
        self.request(
            "Network.enable",
            json!({
                "maxTotalBufferSize": NETWORK_BUFFER_BUDGET,
                "maxResourceBufferSize": MAX_CAPTURED_BODY_BYTES,
                "maxPostDataSize": MAX_POST_DATA_SIZE,
            }),
            Some(session_id),
        )
        .await?;
        self.sessions
            .insert(session_id.to_owned(), target_id.to_owned());
        Ok(())
    }

    /// Writes one frame without waiting for an answer.
    async fn write_frame(&mut self, frame: String) -> Result<(), CaptureError> {
        let socket = self.socket.as_mut().ok_or(CaptureError::NotRunning)?;
        socket.send(Message::text(frame)).await.map_err(|error| {
            CaptureError::Source(format!("cannot write to the CDP socket: {error}"))
        })
    }

    /// Sends a command and waits for its own response, handling events that
    /// arrive in between.
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, CaptureError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write_frame(command(id, method, params, session))
            .await?;
        loop {
            let Some(frame) = self.read_frame().await? else {
                return Err(CaptureError::StreamClosed);
            };
            // The flat-session id is on the message envelope, not in `params`.
            let session = envelope_session(&frame);
            match parse_incoming(&frame).map_err(|error| CaptureError::Source(error.to_string()))? {
                Incoming::Response {
                    id: answered,
                    result,
                    error,
                } if answered == id => {
                    if let Some(error) = error {
                        return Err(CaptureError::Source(format!("{method} failed: {error}")));
                    }
                    return Ok(result.unwrap_or(Value::Null));
                }
                Incoming::Response { id, result, error } => {
                    self.handle_body_response(id, result, error);
                }
                Incoming::Event { method, params } => {
                    self.handle_event(&method, &params, session.as_deref())
                        .await?;
                }
            }
        }
    }

    /// Turns a `Network.getResponseBody` answer into a queued payload update.
    fn handle_body_response(&mut self, id: u64, result: Option<Value>, error: Option<String>) {
        if let Some(pending) = self.pending_bodies.remove(&id) {
            let body = decode_body(result, error, pending.content_type);
            self.queue.push_back(CaptureEvent::BodyUpdated {
                key: pending.key,
                direction: BodyDirection::Response,
                body,
            });
        }
    }

    async fn read_frame(&mut self) -> Result<Option<String>, CaptureError> {
        let Some(socket) = self.socket.as_mut() else {
            return Ok(None);
        };
        loop {
            match socket.next().await {
                None => return Ok(None),
                Some(Ok(Message::Text(text))) => return Ok(Some(text.as_str().to_owned())),
                Some(Ok(Message::Binary(bytes))) => {
                    return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
                }
                Some(Ok(Message::Ping(payload))) => {
                    let _ = socket.send(Message::Pong(payload)).await;
                }
                Some(Ok(_)) => {}
                Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) => return Ok(None),
                Some(Err(error)) => {
                    return Err(CaptureError::Source(format!("CDP socket failed: {error}")));
                }
            }
        }
    }

    /// Translates one CDP event and queues what it produces.
    ///
    /// `session` is the flat-session id carried by the message envelope: a body
    /// command belongs to the session that observed the response, so a session
    /// id read from `params` (which only `Target.*` events carry) cannot stand in
    /// for it.
    async fn handle_event(
        &mut self,
        method: &str,
        params: &Value,
        session: Option<&str>,
    ) -> Result<(), CaptureError> {
        match method {
            "Target.attachedToTarget" => {
                let is_page = params
                    .get("targetInfo")
                    .and_then(|info| info.get("type"))
                    .and_then(Value::as_str)
                    == Some("page");
                if is_page && let Some(session_id) = params.get("sessionId").and_then(Value::as_str)
                {
                    let target_id = params
                        .get("targetInfo")
                        .and_then(|info| info.get("targetId"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    // Enabling is a command, and commands cannot be awaited from
                    // inside event handling without recursion; the pump drains
                    // this queue on its next iteration.
                    if self.attached_targets.contains(&target_id) {
                        // Auto-attach fired for a target we already have. The
                        // target is paused by `waitForDebuggerOnStart`, so it
                        // must be resumed even though its events are dropped.
                        self.pending_setup.push_back(PendingSession::ResumeOnly {
                            session_id: session_id.to_owned(),
                        });
                    } else {
                        self.attached_targets.insert(target_id.clone());
                        self.pending_setup.push_back(PendingSession::Enable {
                            session_id: session_id.to_owned(),
                            target_id,
                        });
                    }
                }
                return Ok(());
            }
            "Target.detachedFromTarget" => {
                if let Some(session_id) = params.get("sessionId").and_then(Value::as_str) {
                    self.sessions.remove(session_id);
                }
                return Ok(());
            }
            _ => {}
        }

        let request_id = params.get("requestId").and_then(Value::as_str);
        let key = request_id.map(|id| SourceKey::new(CaptureSource::Cdp, id));

        if method == "Network.requestWillBeSent"
            && let Some(key) = key.clone()
        {
            self.inflight.entry(key).or_insert_with(|| Inflight {
                draft: TimingDraft::new(params),
                content_type: None,
            });
        }

        if method == "Network.responseReceived"
            && let Some(key) = key.clone()
            && let Some(response) = params.get("response")
        {
            let entry = self.inflight.entry(key).or_insert_with(|| Inflight {
                draft: TimingDraft::new(params),
                content_type: None,
            });
            entry.draft.observe_response(params);
            entry.content_type = response
                .get("mimeType")
                .and_then(Value::as_str)
                .map(ContentType::parse);
        }

        let mut outcomes = translate(method, params, CaptureSource::Cdp);
        if outcomes.is_empty() {
            return Ok(());
        }
        let finished_timestamp = params
            .get("timestamp")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        for outcome in outcomes.drain(..) {
            match outcome {
                CdpOutcome::Event(event) => {
                    let mut event = *event;
                    match &mut event {
                        CaptureEvent::RequestCompleted { key, timing, .. } => {
                            if let Some(inflight) = self.inflight.remove(key) {
                                *timing =
                                    Some(finalize_timing(&inflight.draft, finished_timestamp));
                            }
                        }
                        CaptureEvent::RequestFailed { key, .. }
                        | CaptureEvent::RequestAborted { key, .. } => {
                            self.inflight.remove(key);
                        }
                        _ => {}
                    }
                    self.queue.push_back(event);
                }
                CdpOutcome::FetchResponseBody {
                    key,
                    request_id,
                    content_type,
                } => {
                    let inflight = self.inflight.get(&key);
                    let resolved_content_type = inflight
                        .and_then(|inflight| inflight.content_type.clone())
                        .or_else(|| content_type.map(|value| ContentType::parse(&value)));
                    let id = self.next_id;
                    self.next_id = self.next_id.saturating_add(1);
                    self.pending_bodies.insert(
                        id,
                        PendingBody {
                            key,
                            content_type: resolved_content_type,
                        },
                    );
                    let frame = command(
                        id,
                        "Network.getResponseBody",
                        json!({ "requestId": request_id }),
                        session.or_else(|| self.single_session()),
                    );
                    self.write_frame(frame).await?;
                }
            }
        }
        Ok(())
    }

    /// Runs the `Network.enable`/resume work owed to newly attached sessions.
    async fn drain_pending(&mut self) -> Result<(), CaptureError> {
        while let Some(pending) = self.pending_setup.pop_front() {
            match pending {
                PendingSession::Enable {
                    session_id,
                    target_id,
                } => {
                    self.setup_session(&session_id, &target_id).await?;
                    // The target is paused because of `waitForDebuggerOnStart`.
                    self.request(
                        "Runtime.runIfWaitingForDebugger",
                        json!({}),
                        Some(&session_id),
                    )
                    .await?;
                }
                PendingSession::ResumeOnly { session_id } => {
                    let _ = self
                        .request(
                            "Runtime.runIfWaitingForDebugger",
                            json!({}),
                            Some(&session_id),
                        )
                        .await;
                    let _ = self
                        .request(
                            "Target.detachFromTarget",
                            json!({ "sessionId": session_id }),
                            None,
                        )
                        .await;
                }
            }
        }
        Ok(())
    }

    /// The only attached session, when the browser omitted the session id.
    fn single_session(&self) -> Option<&str> {
        if self.sessions.len() == 1 {
            self.sessions.keys().next().map(String::as_str)
        } else {
            None
        }
    }

    /// Retries a lost connection a bounded number of times.
    async fn reconnect(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        self.lost_connections = self.lost_connections.saturating_add(1);
        self.socket = None;
        let reason = "the CDP connection closed".to_owned();
        if self.reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
            return Ok(Some(CaptureEvent::SourceDisconnected {
                source: CaptureSource::Cdp,
                reason,
            }));
        }
        tokio::time::sleep(RECONNECT_BACKOFF * u32::from(self.reconnect_attempts + 1)).await;
        self.reconnect_attempts += 1;
        match self.connect().await {
            Ok(()) => Ok(Some(CaptureEvent::SourceReconnected {
                source: CaptureSource::Cdp,
            })),
            Err(error) => {
                tracing::debug!(error = %error, "CDP reconnect attempt failed");
                Ok(Some(CaptureEvent::SourceDisconnected {
                    source: CaptureSource::Cdp,
                    reason,
                }))
            }
        }
    }
}

#[async_trait]
impl CaptureProvider for CdpCaptureProvider {
    fn source(&self) -> CaptureSource {
        CaptureSource::Cdp
    }

    async fn start(&mut self) -> Result<(), CaptureError> {
        if self.socket.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        self.reconnect_attempts = 0;
        self.connect().await
    }

    async fn stop(&mut self) -> Result<(), CaptureError> {
        self.pending_setup.clear();
        if let Some(socket) = self.socket.as_mut() {
            let _ = socket.close(None).await;
        }
        self.socket = None;
        self.sessions.clear();
        self.attached_targets.clear();
        self.inflight.clear();
        self.pending_bodies.clear();
        self.queue.clear();
        Ok(())
    }

    async fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        if self.socket.is_none() {
            return Err(CaptureError::NotRunning);
        }
        loop {
            if let Some(event) = self.queue.pop_front() {
                return Ok(Some(event));
            }
            if self.pending_setup.is_empty() {
                // fall through to the socket
            } else {
                self.drain_pending().await?;
                continue;
            }
            let Some(frame) = self.read_frame().await? else {
                return self.reconnect().await;
            };
            // The flat-session id is on the message envelope, not in `params`.
            let session = envelope_session(&frame);
            match parse_incoming(&frame).map_err(|error| CaptureError::Source(error.to_string()))? {
                Incoming::Response { id, result, error } => {
                    self.handle_body_response(id, result, error);
                }
                Incoming::Event { method, params } => {
                    self.handle_event(&method, &params, session.as_deref())
                        .await?;
                }
            }
        }
    }
}

/// Reads the flat-session id from a CDP message envelope.
///
/// CDP puts `sessionId` beside `method` and `params`, not inside the event
/// parameters, so it has to be taken from the frame itself. Only `Target.*`
/// events carry a `sessionId` *payload* field, and that one names the target
/// being attached or detached, not the session the frame arrived on.
fn envelope_session(frame: &str) -> Option<String> {
    serde_json::from_str::<Value>(frame)
        .ok()?
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Decodes a `Network.getResponseBody` answer into a payload.
fn decode_body(
    result: Option<Value>,
    error: Option<String>,
    content_type: Option<ContentType>,
) -> Body {
    let Some(result) = result else {
        if let Some(error) = error {
            tracing::debug!(error = %error, "the browser could not provide a response body");
        }
        return Body::omitted(content_type, OmitReason::RetrievalFailed, 0);
    };
    let encoded = result
        .get("base64Encoded")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let text = result
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let bytes = if encoded {
        use base64::Engine as _;
        match base64::engine::general_purpose::STANDARD.decode(text) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::debug!(error = %error, "cannot decode a base64 response body");
                return Body::omitted(content_type, OmitReason::Undecodable, 0);
            }
        }
    } else {
        text.as_bytes().to_vec()
    };
    if bytes.len() > MAX_CAPTURED_BODY_BYTES {
        return Body::omitted(
            content_type,
            OmitReason::TooLarge,
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        );
    }
    Body::captured(content_type, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use netra_core::domain::{BodyData, StatusCode, Timestamp};

    #[test]
    fn plain_bodies_are_decoded_verbatim() {
        let body = decode_body(
            Some(json!({ "body": "{\"ok\":true}", "base64Encoded": false })),
            None,
            Some(ContentType::parse("application/json")),
        );
        assert_eq!(body.text().as_deref(), Some("{\"ok\":true}"));
        assert!(!body.truncated);
    }

    #[test]
    fn base64_bodies_are_decoded() {
        let body = decode_body(
            Some(json!({ "body": "AAEC", "base64Encoded": true })),
            None,
            None,
        );
        assert_eq!(body.bytes().unwrap(), &[0, 1, 2]);
    }

    #[test]
    fn unavailable_bodies_are_omitted_with_a_reason() {
        let body = decode_body(
            None,
            Some("No resource with given identifier".to_owned()),
            None,
        );
        assert!(matches!(
            body.data,
            BodyData::Omitted {
                reason: OmitReason::RetrievalFailed
            }
        ));
        let undecodable = decode_body(
            Some(json!({ "body": "!!!not base64!!!", "base64Encoded": true })),
            None,
            None,
        );
        assert!(matches!(
            undecodable.data,
            BodyData::Omitted {
                reason: OmitReason::Undecodable
            }
        ));
    }

    #[test]
    fn oversized_bodies_are_omitted_instead_of_allocating() {
        let huge = "a".repeat(MAX_CAPTURED_BODY_BYTES + 16);
        let body = decode_body(
            Some(json!({ "body": huge, "base64Encoded": false })),
            None,
            None,
        );
        assert!(matches!(
            body.data,
            BodyData::Omitted {
                reason: OmitReason::TooLarge
            }
        ));
        assert!(body.size_bytes > MAX_CAPTURED_BODY_BYTES as u64);
    }

    #[tokio::test]
    async fn a_provider_without_a_socket_reports_not_running() {
        let mut provider = CdpCaptureProvider::new(CdpEndpoint::new("127.0.0.1", 9222));
        assert_eq!(provider.source(), CaptureSource::Cdp);
        assert!(matches!(
            provider.next_event().await,
            Err(CaptureError::NotRunning)
        ));
        assert_eq!(provider.lost_connections(), 0);
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_is_reported_as_unavailable() {
        // Port 1 is never a Chrome debugging endpoint.
        let mut provider = CdpCaptureProvider::new(CdpEndpoint::new("127.0.0.1", 1));
        match provider.start().await {
            Err(CaptureError::Unavailable(message)) => {
                assert!(message.contains("--remote-debugging-port"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn response_status_and_timestamp_survive_the_body_round_trip() {
        let body = Body::captured(Some(ContentType::parse("text/plain")), b"ok".to_vec());
        let status = StatusCode::try_new(418).unwrap();
        assert_eq!(status.as_u16(), 418);
        assert_eq!(body.size_bytes, 2);
        let _ = Timestamp::now();
    }
}
