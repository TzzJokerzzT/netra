//! Minimal HTTP/1.1 reading for the proxy.
//!
//! The proxy forwards bytes, so this module only understands what it must:
//! the request line, header lines in order, `Content-Length` and chunked
//! bodies. Everything else is rejected with a clear status instead of being
//! guessed at.

use netra_core::domain::HttpHeaders;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Any bidirectional byte stream: plain TCP, TLS, or a boxed combination.
pub trait AsyncIo: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> AsyncIo for T {}

/// Boxed byte stream used for upstream connections.
pub type BoxedStream = Box<dyn AsyncIo + Send + Unpin>;

/// Largest accepted header block.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Proxy-side failures.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// The connection failed or closed unexpectedly.
    #[error("connection error: {0}")]
    Io(#[from] std::io::Error),
    /// The peer sent something that is not HTTP/1.1.
    #[error("malformed HTTP message: {0}")]
    Malformed(String),
    /// The message exceeded a configured limit.
    #[error("{0}")]
    TooLarge(String),
    /// The proxy refuses to perform this operation.
    #[error("{0}")]
    Refused(String),
}

/// A parsed request line plus headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    /// HTTP method as sent.
    pub method: String,
    /// Request target: absolute URI for proxying, authority for CONNECT.
    pub target: String,
    /// Protocol version as sent.
    pub version: String,
    /// Headers in order, duplicates preserved.
    pub headers: HttpHeaders,
}

impl RequestHead {
    /// `true` for a CONNECT request.
    #[must_use]
    pub fn is_connect(&self) -> bool {
        self.method.eq_ignore_ascii_case("CONNECT")
    }

    /// Parses the `host:port` authority of a CONNECT request.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::Malformed`] when the authority has no usable port.
    pub fn connect_authority(&self) -> Result<(String, u16), ProxyError> {
        let (host, port) = self.target.rsplit_once(':').ok_or_else(|| {
            ProxyError::Malformed(format!("CONNECT without port: {}", self.target))
        })?;
        let port: u16 = port
            .parse()
            .map_err(|_| ProxyError::Malformed(format!("invalid CONNECT port: {port}")))?;
        let host = host.trim_matches(|c| c == '[' || c == ']').to_owned();
        if host.is_empty() {
            return Err(ProxyError::Malformed("CONNECT without host".to_owned()));
        }
        Ok((host, port))
    }

    /// `true` when the client asked to keep the connection open.
    #[must_use]
    pub fn wants_keep_alive(&self) -> bool {
        match self.headers.get("connection") {
            Some(value) => !value.eq_ignore_ascii_case("close"),
            None => self.version != "HTTP/1.0",
        }
    }
}

/// A parsed status line plus headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHead {
    /// Protocol version as sent.
    pub version: String,
    /// Status code.
    pub status: u16,
    /// Reason phrase as sent.
    pub reason: String,
    /// Headers in order, duplicates preserved.
    pub headers: HttpHeaders,
}

impl ResponseHead {
    /// Renders the status line and headers.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut text = format!("{} {} {}\r\n", self.version, self.status, self.reason);
        for entry in self.headers.iter() {
            text.push_str(&entry.name);
            text.push_str(": ");
            text.push_str(&entry.value);
            text.push_str("\r\n");
        }
        text.push_str("\r\n");
        text.into_bytes()
    }
}

/// Parses a request head from its text (without the blank line).
///
/// # Errors
///
/// Returns [`ProxyError::Malformed`] when the request line or a header is
/// unreadable.
pub fn parse_request_head(text: &str) -> Result<RequestHead, ProxyError> {
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| ProxyError::Malformed("empty request".to_owned()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| ProxyError::Malformed(format!("bad request line: {request_line}")))?
        .to_owned();
    let target = parts
        .next()
        .ok_or_else(|| ProxyError::Malformed(format!("bad request line: {request_line}")))?
        .to_owned();
    let version = parts
        .next()
        .ok_or_else(|| ProxyError::Malformed(format!("bad request line: {request_line}")))?
        .to_owned();
    if !version.starts_with("HTTP/1.") {
        return Err(ProxyError::Malformed(format!(
            "unsupported version: {version}"
        )));
    }
    Ok(RequestHead {
        method,
        target,
        version,
        headers: parse_headers(lines)?,
    })
}

/// Parses a response head from its text.
///
/// # Errors
///
/// Returns [`ProxyError::Malformed`] for unreadable status lines or headers.
pub fn parse_response_head(text: &str) -> Result<ResponseHead, ProxyError> {
    let mut lines = text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| ProxyError::Malformed("empty response".to_owned()))?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default().to_owned();
    let status: u16 = parts
        .next()
        .ok_or_else(|| ProxyError::Malformed(format!("bad status line: {status_line}")))?
        .parse()
        .map_err(|_| ProxyError::Malformed(format!("bad status code: {status_line}")))?;
    let reason = parts.next().unwrap_or_default().to_owned();
    Ok(ResponseHead {
        version,
        status,
        reason,
        headers: parse_headers(lines)?,
    })
}

fn parse_headers<'a>(lines: impl Iterator<Item = &'a str>) -> Result<HttpHeaders, ProxyError> {
    let mut headers = HttpHeaders::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ProxyError::Malformed(format!("bad header line: {line}")));
        };
        headers.push(name.trim(), value.trim());
    }
    Ok(headers)
}

/// A stream with a read-ahead buffer, so the proxy can parse a head and then
/// hand the same bytes to the body reader.
#[derive(Debug)]
pub struct Connection<S> {
    inner: S,
    buffer: Vec<u8>,
}

impl<S> Connection<S> {
    /// Wraps a stream.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
        }
    }

    /// Unwraps the stream, returning any unread buffered bytes.
    pub fn into_parts(self) -> (S, Vec<u8>) {
        (self.inner, self.buffer)
    }

    /// Mutable access to the underlying stream.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl<S: AsyncRead + Unpin> Connection<S> {
    /// Reads one head, returning its text without the terminating blank line.
    ///
    /// Returns `Ok(None)` when the peer closed the connection before sending
    /// anything.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::TooLarge`] when the head exceeds
    /// [`MAX_HEAD_BYTES`].
    pub async fn read_head(&mut self) -> Result<Option<String>, ProxyError> {
        loop {
            if let Some(index) = find_double_crlf(&self.buffer) {
                let head = String::from_utf8_lossy(&self.buffer[..index]).into_owned();
                self.buffer.drain(..index + 4);
                return Ok(Some(head));
            }
            if self.buffer.len() > MAX_HEAD_BYTES {
                return Err(ProxyError::TooLarge(format!(
                    "request headers exceed {MAX_HEAD_BYTES} bytes"
                )));
            }
            let read = self.read_more().await?;
            if read == 0 {
                return if self.buffer.is_empty() {
                    Ok(None)
                } else {
                    Err(ProxyError::Malformed("truncated head".to_owned()))
                };
            }
        }
    }

    /// Reads a full body according to `Content-Length` or chunked encoding.
    ///
    /// Returns `Ok(None)` when the message declares no body.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::TooLarge`] when the body exceeds `limit`, and
    /// [`ProxyError::Malformed`] for broken chunk framing.
    pub async fn read_body(
        &mut self,
        headers: &HttpHeaders,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, ProxyError> {
        if headers
            .get("transfer-encoding")
            .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
        {
            return self.read_chunked(limit).await.map(Some);
        }
        let Some(length) = headers.get("content-length") else {
            return Ok(None);
        };
        let length: usize = length
            .trim()
            .parse()
            .map_err(|_| ProxyError::Malformed(format!("invalid content-length: {length}")))?;
        if length == 0 {
            return Ok(None);
        }
        if length > limit {
            return Err(ProxyError::TooLarge(format!(
                "body of {length} bytes exceeds the configured limit of {limit}"
            )));
        }
        while self.buffer.len() < length {
            if self.read_more().await? == 0 {
                return Err(ProxyError::Malformed("truncated body".to_owned()));
            }
        }
        let body: Vec<u8> = self.buffer.drain(..length).collect();
        Ok(Some(body))
    }

    async fn read_chunked(&mut self, limit: usize) -> Result<Vec<u8>, ProxyError> {
        let mut body = Vec::new();
        loop {
            let line = self.read_line().await?;
            let size_text = line.split(';').next().unwrap_or_default().trim();
            let size = usize::from_str_radix(size_text, 16)
                .map_err(|_| ProxyError::Malformed(format!("invalid chunk size: {line}")))?;
            if size == 0 {
                // Consume trailers.
                loop {
                    let trailer = self.read_line().await?;
                    if trailer.is_empty() {
                        return Ok(body);
                    }
                }
            }
            if body.len() + size > limit {
                return Err(ProxyError::TooLarge(format!(
                    "chunked body exceeds the configured limit of {limit}"
                )));
            }
            while self.buffer.len() < size + 2 {
                if self.read_more().await? == 0 {
                    return Err(ProxyError::Malformed("truncated chunk".to_owned()));
                }
            }
            body.extend(self.buffer.drain(..size));
            // Trailing CRLF after the chunk payload.
            if self.buffer.len() < 2 && self.read_more().await? == 0 {
                return Err(ProxyError::Malformed(
                    "truncated chunk terminator".to_owned(),
                ));
            }
            self.buffer.drain(..2);
        }
    }

    async fn read_line(&mut self) -> Result<String, ProxyError> {
        loop {
            if let Some(index) = find_crlf(&self.buffer) {
                let line = String::from_utf8_lossy(&self.buffer[..index]).into_owned();
                self.buffer.drain(..index + 2);
                return Ok(line);
            }
            if self.buffer.len() > MAX_HEAD_BYTES {
                return Err(ProxyError::TooLarge("protocol line too long".to_owned()));
            }
            if self.read_more().await? == 0 {
                return Err(ProxyError::Malformed("truncated protocol line".to_owned()));
            }
        }
    }

    async fn read_more(&mut self) -> Result<usize, ProxyError> {
        let mut chunk = [0u8; 8192];
        let read = self.inner.read(&mut chunk).await?;
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(read)
    }
}

impl<S: AsyncWrite + Unpin> Connection<S> {
    /// Half-closes the stream, sending a TLS `close_notify` when applicable.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::Io`] when the shutdown fails.
    pub async fn shutdown(&mut self) -> Result<(), ProxyError> {
        self.inner.shutdown().await?;
        Ok(())
    }

    /// Writes bytes to the underlying stream.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::Io`] when the write fails.
    pub async fn write_all(&mut self, bytes: &[u8]) -> Result<(), ProxyError> {
        self.inner.write_all(bytes).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

fn find_double_crlf(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn find_crlf(buffer: &[u8]) -> Option<usize> {
    buffer.windows(2).position(|window| window == b"\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_heads_keep_order_and_duplicates() {
        let head = parse_request_head(
            "GET http://example.com/a?b=1 HTTP/1.1\r\nHost: example.com\r\nX-A: 1\r\nX-A: 2",
        )
        .unwrap();
        assert_eq!(head.method, "GET");
        assert_eq!(head.target, "http://example.com/a?b=1");
        assert_eq!(head.headers.get_all("x-a").count(), 2);
        assert!(head.wants_keep_alive());
    }

    #[test]
    fn connect_authorities_are_parsed_without_ambiguity() {
        let head = parse_request_head("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443")
            .unwrap();
        assert!(head.is_connect());
        assert_eq!(
            head.connect_authority().unwrap(),
            ("example.com".to_owned(), 443)
        );
        let bracketed = parse_request_head("CONNECT [::1]:8443 HTTP/1.1").unwrap();
        assert_eq!(
            bracketed.connect_authority().unwrap(),
            ("::1".to_owned(), 8443)
        );
        let portless = parse_request_head("CONNECT example.com HTTP/1.1").unwrap();
        assert!(portless.connect_authority().is_err());
    }

    #[test]
    fn malformed_heads_are_rejected() {
        assert!(parse_request_head("").is_err());
        assert!(parse_request_head("GET /only-a-path").is_err());
        assert!(parse_request_head("GET / HTTP/2").is_err());
        assert!(parse_request_head("GET / HTTP/1.1\r\nno-colon-here").is_err());
        assert!(parse_response_head("HTTP/1.1 abc OK").is_err());
    }

    #[test]
    fn response_heads_round_trip() {
        let head =
            parse_response_head("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nSet-Cookie: a=1")
                .unwrap();
        assert_eq!(head.status, 404);
        let rendered = String::from_utf8(head.to_bytes()).unwrap();
        assert!(rendered.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(rendered.ends_with("\r\n\r\n"));
        assert!(rendered.contains("Set-Cookie: a=1\r\n"));
    }

    #[tokio::test]
    async fn bodies_are_read_by_content_length() {
        let mut connection = Connection::new(&b"hello world"[..]);
        let headers = HttpHeaders::from_pairs([("Content-Length", "5")]);
        let body = connection.read_body(&headers, 100).await.unwrap();
        assert_eq!(body, Some(b"hello".to_vec()));
    }

    #[tokio::test]
    async fn oversized_bodies_are_refused_before_buffering() {
        let mut connection = Connection::new(&b"0123456789"[..]);
        let headers = HttpHeaders::from_pairs([("Content-Length", "10")]);
        assert!(matches!(
            connection.read_body(&headers, 4).await,
            Err(ProxyError::TooLarge(_))
        ));
    }

    #[tokio::test]
    async fn chunked_bodies_are_decoded() {
        let raw = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        let mut connection = Connection::new(&raw[..]);
        let headers = HttpHeaders::from_pairs([("Transfer-Encoding", "chunked")]);
        let body = connection.read_body(&headers, 1024).await.unwrap();
        assert_eq!(body, Some(b"Wikipedia".to_vec()));
    }

    #[tokio::test]
    async fn bodies_absent_are_reported_as_none() {
        let mut connection = Connection::new(&b""[..]);
        assert_eq!(
            connection.read_body(&HttpHeaders::new(), 10).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn heads_stop_at_the_blank_line_and_leave_the_body_buffered() {
        let mut connection = Connection::new(&b"GET / HTTP/1.1\r\nHost: a\r\n\r\nBODY"[..]);
        let head = connection.read_head().await.unwrap().unwrap();
        assert_eq!(head, "GET / HTTP/1.1\r\nHost: a");
        let (_, rest) = connection.into_parts();
        assert_eq!(rest, b"BODY");
    }
}
