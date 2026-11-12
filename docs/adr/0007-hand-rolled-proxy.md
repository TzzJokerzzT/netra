# ADR 0007 — Hand-rolled HTTP/1.1 proxy with an explicit local authority

Status: accepted (milestone M4).

## Context

The proxy source must observe traffic from browsers and tools that accept a
proxy, including HTTPS when the user allows decryption. The brief named
`hudsucker` as one option and required a locally generated authority, private key
handling, loopback-only binding, limits and honest documentation of what cannot
be intercepted.

## Options considered

| Option | Assessment |
| --- | --- |
| `hudsucker` (MITM proxy library) | Complete, but it owns the request/response model, its own CA handling and a large dependency tree. The adapter still has to translate everything into `CaptureEvent`, and the limits/redaction policy would live in two places. |
| `hyper` 1.x as a server | Solid HTTP implementation, but a proxy also needs CONNECT tunnelling, conditional TLS termination and byte-exact framing control — more plumbing than protocol. |
| Hand-written HTTP/1.1 on `tokio` | Fits the existing pipeline exactly, keeps one policy (limits + redaction) in `netra-core`, and the parser is small because the proxy only needs request line, headers, `Content-Length` and chunked bodies. |

## Decision

Hand-roll HTTP/1.1 in `netra-capture/proxy`, with `rustls`/`tokio-rustls`/`rcgen`
for TLS and the authority. Configuration stays in `netra-core` so validation,
limits and redaction are shared with the CDP source.

Deliberate scope decisions, each visible in code and docs:

- **One exchange per connection** (`Connection: close`). Persistent connections
  and pipelining are refused rather than half-implemented; framing stays
  unambiguous.
- **Bodies are bounded**: `proxy.max_body_bytes` (8 MiB default). A larger body
  is refused with a closed connection instead of being buffered or silently
  truncated.
- **CONNECT without interception is a tunnel**: bytes are relayed and the
  exchange is reported as a tunnel with no invented response.
- **Upstream TLS is verified** with the Mozilla root store plus explicitly
  configured `proxy.extra_trust_roots`. Validation is never disabled.
- **Loopback only** unless `proxy.allow_remote` is set, enforced both by
  configuration validation and by the adapter itself.

## Consequences

- The authority is created only by `netra ca init`, its key is `0600`, nothing
  is installed anywhere, and `netra ca show` reports the PEM digest so a user can
  compare it in the browser before trusting it.
- Interception requires the operator to trust the CA. That is the honest cost of
  decryption and is documented instead of hidden behind a flag.
- HTTP/2 upstream is not negotiated; intercepted sessions speak HTTP/1.1 to
  origins. ALPN/h2 support is future work.
- A malformed or oversized message costs one connection, never the listener.
