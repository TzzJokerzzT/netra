# ADR 0006 — A small in-tree CDP client instead of a generated binding crate

Status: accepted (milestone M3).

## Context

`netra` needs to observe real Chrome/Chromium traffic. The DevTools Protocol is
JSON-RPC over a WebSocket, and the project brief suggested `chromiumoxide` or
another maintained CDP implementation.

What the inspector actually needs is narrow:

1. Discover the browser endpoint and its page targets (`/json/version`,
   `Target.setAutoAttach`).
2. Attach to each page as a flat session and enable `Network`.
3. Translate a handful of `Network.*` events into `CaptureEvent`s.
4. Ask for response bodies (`Network.getResponseBody`) without blocking the
   event pump.

Everything else — correlation, limits, redaction, persistence — already lives in
the shared pipeline, so a provider's job is translation.

## Options considered

| Option | Assessment |
| --- | --- |
| `chromiumoxide` 0.9.1 (updated 2026-02, 5.7M downloads) | Maintained and complete, but it embeds the entire generated protocol surface (every CDP domain and type). That is a large compile-time and dependency commitment for four methods, and its typed events would be partly discarded when normalising to `CaptureEvent`. |
| `headless_chrome` 1.0.22 | Maintained, but a synchronous API model designed around driving one page; a poor fit for a pipeline that must keep several targets streaming while other work continues. |
| `browser-protocol` 0.1.6 | Generated types only, very low adoption (583 downloads) — a single-maintainer dependency for a security-relevant surface. |
| Direct client on `tokio-tungstenite` + `serde_json` | Two widely used, stable dependencies; full control over sessions, body fetching and failure handling; the translation layer becomes testable with JSON fixtures. Cost: ~900 lines to own. |

## Decision

Implement a small CDP client inside `netra-capture/cdp`, using
`tokio-tungstenite` for the socket and `serde_json` for the wire format. Keep it
in four files with one responsibility each: `protocol` (frames), `discovery`
(targets), `mapping` (pure translation, unit tested against fixtures),
`provider` (lifecycle, sessions, body fetch, bounded reconnect).

Revisit the decision if CDP domain coverage grows far beyond the `Network`
domain (WebSockets, Fetch interception, heap snapshots): at that point a
generated binding becomes cheaper than hand-written translation.

## Consequences

- Two dependency edges instead of a large crate; the workspace builds quickly
  and `cargo audit` sees a small surface.
- The translation layer is pure and covered by fixtures, so protocol shape
  changes surface as test failures instead of silent gaps.
- We own the protocol details, including its sharp edges: paused targets
  (`waitForDebuggerOnStart`), flat session ids, duplicate `Set-Cookie`
  collapsing in `response.headers`, and bodies that are simply unavailable.
- A breaking CDP change is ours to fix; the `Network` domain is one of the most
  stable parts of the protocol, which bounds that risk.
