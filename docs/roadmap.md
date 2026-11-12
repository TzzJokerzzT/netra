# Roadmap

State of every milestone. A milestone is only "done" when its behaviour is
implemented, tested and documented here.

## M0 — Foundation — done

Workspace with seven crates, domain model and ports, Clap CLI, tracing,
versioned CI (fmt, clippy `-D warnings`, tests).

## M1 — TUI foundation — done

Ratatui/crossterm layout that adapts from 40x12 upwards, request list, inspector
with seven tabs, filters, search, help overlay, confirmation dialog, keyboard
navigation, `TestBackend` tests.

## M2 — Storage engine — done

SQLite with WAL, foreign keys, versioned migrations, private permissions,
incremental persistence per processor update, paging and filtering in SQL,
retention, crash recovery of sessions left active, restart tests.

## M3 — CDP capture — done

Implemented: target discovery, auto-attach with `waitForDebuggerOnStart`,
flat sessions per page, `Network.enable`, correlation of
`requestWillBeSent`/`responseReceived`/`loadingFinished`/`loadingFailed`,
redirect chains, `Network.getResponseBody` (base64 and unavailable bodies),
phase timings, bounded reconnect, and an integration test that drives a real
Chromium against a local fixture. Decision recorded in ADR 0006.

Deliberate gaps, each documented in `docs/architecture.md`:

- `requestWillBeSentExtraInfo` / `responseReceivedExtraInfo` are ignored, so
  duplicate response headers the protocol collapsed stay collapsed.
- WebSockets are not inspected (M6).
- The adapter attaches to an already-running browser; it never launches one.
  Start Chrome with `--remote-debugging-port=9222`.

## M4 — HTTP/HTTPS proxy — not started

Needs: a forward proxy with CONNECT (evaluate `hudsucker` and alternatives),
local certificate authority generation with `rcgen`/`rustls`, per-host leaf
certificates, private key permissions, tunnel-without-decryption mode,
connection/body/timeout limits, and tests against local fixture servers
including a TLS one.

## M5 — Analysis depth — partially done

Implemented: JSON pretty-printing, media classification, hex previews, timing
waterfall data, error insights, header highlights, session statistics.

Remaining: statistics screen in the TUI, richer search UX, comparison view.

## M6 — Advanced features — not started

Safe replay with SSRF guards, HAR import, exchange comparison, WebSocket
inspection, replay-driven regression checks. HAR **export** already exists.

## M7 — Hardening and release — not started

Concurrency and stress tests (1 000 concurrent exchanges, slow storage),
recovery tests under kill -9, security review, memory/storage profiling,
installation documentation, binary distribution, `0.1.0` release criteria from
the project brief.
