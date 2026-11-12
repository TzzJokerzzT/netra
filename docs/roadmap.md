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

## M3 — CDP capture — not started

Needs: a maintained CDP client (evaluate `chromiumoxide` and alternatives for
maintenance status and tokio support, record the choice in an ADR), WebSocket
target discovery, `Network.enable`, correlation of
`requestWillBeSent`/`requestWillBeSentExtraInfo`/`responseReceived`/
`responseReceivedExtraInfo`/`loadingFinished`/`loadingFailed`,
`Network.getResponseBody` with base64 handling and "not available" bodies,
redirect chains, target navigation, reconnect without corrupting the session,
and an integration test against a real Chromium instance.

Until it exists, `netra capture --source cdp` exits with code 3 and says which
milestone is missing.

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
