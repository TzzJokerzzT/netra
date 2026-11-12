//! Turn status codes and transport failures into something actionable.

use netra_core::domain::{ExchangeState, NetworkExchange, StatusCode};

/// Category of an observed error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 401/403
    Authorization,
    /// 404/410
    NotFound,
    /// 429
    RateLimited,
    /// 408/504
    Timeout,
    /// 5xx other than 504
    ServerError,
    /// Transport failure reported by the capture source.
    Transport,
    /// Aborted by the client or by navigation.
    Aborted,
    /// 3xx loop or unexpected redirect
    Redirect,
    /// Any other 4xx
    ClientError,
}

/// Actionable summary of a failed exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorInsight {
    /// Category.
    pub kind: ErrorKind,
    /// One-line diagnosis.
    pub summary: String,
    /// What to check next.
    pub suggestion: String,
}

/// Builds an insight for a failed, aborted or error-status exchange.
#[must_use]
pub fn insight(exchange: &NetworkExchange) -> Option<ErrorInsight> {
    let detail = exchange.error.clone().unwrap_or_default();
    match (exchange.state, exchange.status()) {
        (ExchangeState::Failed, _) => Some(insight_for_transport(&detail)),
        (ExchangeState::Aborted, _) => Some(ErrorInsight {
            kind: ErrorKind::Aborted,
            summary: "request was aborted before completion".to_owned(),
            suggestion: "check whether navigation or a client-side cancellation caused it"
                .to_owned(),
        }),
        (_, Some(status)) => insight_for_status(status),
        _ => None,
    }
}

fn insight_for_status(status: StatusCode) -> Option<ErrorInsight> {
    let insight = match status.as_u16() {
        401 | 403 => ErrorInsight {
            kind: ErrorKind::Authorization,
            summary: format!("authorization rejected ({status})"),
            suggestion: "verify credentials, scopes and the Authorization header".to_owned(),
        },
        404 | 410 => ErrorInsight {
            kind: ErrorKind::NotFound,
            summary: format!("resource missing ({status})"),
            suggestion: "check the path, base URL and API version".to_owned(),
        },
        408 | 504 => ErrorInsight {
            kind: ErrorKind::Timeout,
            summary: format!("timeout ({status})"),
            suggestion: "check upstream latency and proxy timeouts".to_owned(),
        },
        429 => ErrorInsight {
            kind: ErrorKind::RateLimited,
            summary: "rate limited (429)".to_owned(),
            suggestion: "inspect Retry-After and reduce request rate".to_owned(),
        },
        500..=599 => ErrorInsight {
            kind: ErrorKind::ServerError,
            summary: format!("server error ({status})"),
            suggestion: "check the server log and whether the payload is valid".to_owned(),
        },
        300..=399 => ErrorInsight {
            kind: ErrorKind::Redirect,
            summary: format!("redirect ({status})"),
            suggestion: "confirm the target is expected; follow the redirect chain".to_owned(),
        },
        400..=499 => ErrorInsight {
            kind: ErrorKind::ClientError,
            summary: format!("client error ({status})"),
            suggestion: "check request shape, parameters and content type".to_owned(),
        },
        _ => return None,
    };
    Some(insight)
}

fn insight_for_transport(error: &str) -> ErrorInsight {
    let lower = error.to_lowercase();
    let (summary, suggestion) =
        if lower.contains("cert") || lower.contains("tls") || lower.contains("ssl") {
            (
                "TLS handshake or certificate rejected",
                "check the local CA trust, hostname and certificate pinning",
            )
        } else if lower.contains("refused") {
            (
                "connection refused",
                "check that the target is listening and the port is right",
            )
        } else if lower.contains("timeout") || lower.contains("timed out") {
            (
                "connection timed out",
                "check reachability, DNS and firewall rules",
            )
        } else if lower.contains("dns") || lower.contains("name_not_resolved") {
            (
                "DNS resolution failed",
                "check the hostname and resolver configuration",
            )
        } else if lower.contains("reset") {
            (
                "connection reset by peer",
                "check server-side limits, proxies and keep-alive",
            )
        } else {
            (
                "transport failure",
                "inspect the error text and the source logs",
            )
        };
    ErrorInsight {
        kind: ErrorKind::Transport,
        summary: summary.to_owned(),
        suggestion: suggestion.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netra_core::domain::{
        CaptureSource, HttpMethod, NetworkExchange, NetworkRequest, NetworkResponse, RequestUrl,
        SessionId, Timestamp,
    };

    fn exchange() -> NetworkExchange {
        let request = NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("https://example.com/"),
            Timestamp::now(),
        );
        NetworkExchange::pending(SessionId::new(), CaptureSource::Cdp, request, None)
    }

    #[test]
    fn transport_failures_are_classified_by_message() {
        let mut failed = exchange();
        failed.fail("net::ERR_CERT_AUTHORITY_INVALID");
        let insight = insight(&failed).unwrap();
        assert_eq!(insight.kind, ErrorKind::Transport);
        assert!(insight.suggestion.contains("CA"));
    }

    #[test]
    fn status_codes_map_to_categories() {
        let mut completed = exchange();
        completed.complete(NetworkResponse::new(
            StatusCode::try_new(429).unwrap(),
            Timestamp::now(),
        ));
        assert_eq!(insight(&completed).unwrap().kind, ErrorKind::RateLimited);
        completed.complete(NetworkResponse::new(
            StatusCode::try_new(503).unwrap(),
            Timestamp::now(),
        ));
        assert_eq!(insight(&completed).unwrap().kind, ErrorKind::ServerError);
    }

    #[test]
    fn successful_exchanges_have_no_insight() {
        let mut completed = exchange();
        completed.complete(NetworkResponse::new(
            StatusCode::try_new(204).unwrap(),
            Timestamp::now(),
        ));
        assert!(insight(&completed).is_none());
    }

    #[test]
    fn aborted_exchanges_report_cancellation() {
        let mut aborted = exchange();
        aborted.abort(Some("navigation".into()));
        assert_eq!(insight(&aborted).unwrap().kind, ErrorKind::Aborted);
    }
}
