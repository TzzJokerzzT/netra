//! CDP wire format: JSON-RPC over a WebSocket.
//!
//! Only the shapes this adapter needs are modelled; unknown fields are ignored
//! so a protocol addition cannot break capture.

use serde_json::{Value, json};

/// One incoming frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// Reply to a command we sent.
    Response {
        /// Command identifier.
        id: u64,
        /// Result payload, when the command succeeded.
        result: Option<Value>,
        /// Error payload, when it failed.
        error: Option<String>,
    },
    /// An event emitted by the browser or a session.
    Event {
        /// CDP method name, for example `Network.loadingFinished`.
        method: String,
        /// Event parameters.
        params: Value,
        /// Flat-session id from the message envelope, when the frame carried one.
        ///
        /// CDP puts it beside `method` and `params`, never inside `params`: the
        /// `sessionId` a `Target.*` payload carries names the target being
        /// attached, not the session the frame arrived on.
        session: Option<String>,
    },
}

/// Frames that are not valid protocol messages.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// The frame was not valid JSON.
    #[error("invalid CDP frame: {0}")]
    NotJson(String),
    /// The frame was JSON but neither a response nor an event.
    #[error("unexpected CDP frame: {0}")]
    Unexpected(String),
}

/// Parses one frame into a response or an event.
///
/// # Errors
///
/// Returns [`ProtocolError`] for malformed frames.
pub fn parse_incoming(frame: &str) -> Result<Incoming, ProtocolError> {
    let value: Value =
        serde_json::from_str(frame).map_err(|error| ProtocolError::NotJson(error.to_string()))?;
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        let error = value.get("error").map(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map_or_else(|| error.to_string(), str::to_owned)
        });
        return Ok(Incoming::Response {
            id,
            result: value.get("result").cloned(),
            error,
        });
    }
    match value.get("method").and_then(Value::as_str) {
        Some(method) => Ok(Incoming::Event {
            method: method.to_owned(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
            session: value
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        None => Err(ProtocolError::Unexpected(frame.chars().take(120).collect())),
    }
}

/// Builds a command frame. `session` selects a flat target session.
#[must_use]
pub fn command(id: u64, method: &str, params: Value, session: Option<&str>) -> String {
    let mut frame = json!({ "id": id, "method": method, "params": params });
    if let Some(session) = session
        && let Some(object) = frame.as_object_mut()
    {
        object.insert("sessionId".to_owned(), Value::String(session.to_owned()));
    }
    frame.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_command_response() {
        let frame = r#"{"id":7,"result":{"body":"e30=","base64Encoded":true}}"#;
        match parse_incoming(frame).unwrap() {
            Incoming::Response { id, result, error } => {
                assert_eq!(id, 7);
                assert!(error.is_none());
                assert_eq!(result.unwrap()["body"], "e30=");
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn parses_an_error_response_without_losing_its_message() {
        let frame =
            r#"{"id":3,"error":{"code":-32000,"message":"No resource with given identifier"}}"#;
        match parse_incoming(frame).unwrap() {
            Incoming::Response { id, error, .. } => {
                assert_eq!(id, 3);
                assert_eq!(error.as_deref(), Some("No resource with given identifier"));
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn parses_an_event_and_tolerates_unknown_fields() {
        let frame = r#"{"method":"Network.loadingFinished","params":{"requestId":"1","encodedDataLength":12},"sessionId":"abc","futureField":true}"#;
        match parse_incoming(frame).unwrap() {
            Incoming::Event {
                method,
                params,
                session,
            } => {
                assert_eq!(method, "Network.loadingFinished");
                assert_eq!(params["requestId"], "1");
                assert_eq!(session.as_deref(), Some("abc"));
            }
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn an_event_without_an_envelope_session_carries_none() {
        let frame = r#"{"method":"Target.targetInfoChanged","params":{"sessionId":"payload"}}"#;
        match parse_incoming(frame).unwrap() {
            Incoming::Event { session, .. } => assert!(
                session.is_none(),
                "a `sessionId` inside `params` names the payload, not the envelope"
            ),
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn rejects_frames_that_are_neither() {
        assert!(parse_incoming("not json").is_err());
        assert!(parse_incoming(r#"{"hello":true}"#).is_err());
    }

    #[test]
    fn commands_carry_the_session_when_present() {
        let with = command(1, "Network.enable", json!({}), Some("s1"));
        assert!(with.contains("\"sessionId\":\"s1\""));
        let without = command(2, "Target.getTargets", json!({}), None);
        assert!(!without.contains("sessionId"));
    }
}
