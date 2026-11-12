# Architecture

`netra` is a Cargo workspace with a strict dependency direction: adapters depend
on the domain, never the other way around. The domain crate does not know about
Ratatui, SQLite, Tokio, CDP or the proxy.

```
crates/
├── netra-core           domain entities, value objects, ports  (no infra deps)
├── netra-application    use cases, capture runtime, HAR export
├── netra-capture        event correlation processor + demo provider
├── netra-analysis       body/json/timing/header/error inspection
├── netra-storage        SQLite adapters (sessions, exchanges, bodies)
├── netra-tui            Ratatui widgets, state and key handling
└── netra-cli            clap surface, bootstrap, TUI loop   (binary: netra)
```

Dependency edges:

```
netra-cli ──> netra-tui ──> netra-analysis ──> netra-core
    │             │                              ▲
    ├──> netra-application ──────────────────────┤
    │        │                                   │
    │        ├──> netra-capture ─────────────────┤
    │        └──> (ports)                        │
    └──> netra-storage ──────────────────────────┘
```

`netra-capture` and `netra-storage` never depend on each other; the application
layer is the only place that knows both.

## Domain model

The aggregate is `NetworkExchange`: a request, an optional response, timing,
source and state (`pending`, `completed`, `failed`, `aborted`). A request exists
and is visible before its response, which is what makes a live network list
usable.

Value objects carry their own invariants: `HttpMethod` validates the RFC 9110
token grammar, `StatusCode` rejects values outside 100..=599, `HttpHeaders`
preserves order and duplicates (several `Set-Cookie` lines must survive),
`RequestUrl` keeps the exact captured text even when it does not parse, `Body`
distinguishes absent / omitted / redacted / truncated, and `Timestamp` versus
`DurationMs` keeps instants and elapsed times apart.

## Ports

Defined in `netra-core::ports`:

| Port | Implemented by | Notes |
| --- | --- | --- |
| `CaptureProvider` | `netra-capture` (demo), M3 CDP, M4 proxy | `start` / `stop` / `next_event` |
| `SessionRepository` | `netra-storage` | CRUD, counters, interrupted-session recovery |
| `ExchangeRepository` | `netra-storage` | incremental `apply`, paging, retention, counts |
| `Clock` | `SystemClock`, test doubles | keeps tests deterministic |

## Capture flow

```
CdpCaptureProvider  ─┐
ProxyCaptureProvider ─┤  (one Tokio task per provider, each owning its provider)
                      │        │
                      │        └── bounded mpsc (1024) ── backpressure, no loss
                      v
                 consumer task
                      │  1. pause gate (skips are counted and reported)
                      │  2. CaptureProcessor: correlation by (source, request id)
                      │  3. limits + redaction
                      v
            ExchangeRepository::apply(update)   (one transaction per update)
                      │
                      └── broadcast notifications ──> TUI / CLI progress
```

Design properties enforced by the code and its tests:

- **Capture never blocks rendering.** The UI reads from storage on its own
  cadence; it never holds the capture pipeline.
- **No unbounded memory.** The processor keeps only a key → exchange-id map plus
  a bounded (256) orphan-response cache. Payloads live in SQLite.
- **No silent loss.** Skipped events, dropped orphans, truncated headers and
  bodies, withheld payloads and failed writes are all counters
  (`ProcessorDiagnostics`) or `AppEvent::Warning` notifications.
- **Out-of-order tolerance.** CDP can deliver a response before the request;
  orphan responses are merged when the request arrives. Duplicate starts are
  counted instead of duplicated.

## Persistence

- One connection behind a mutex; every query runs in
  `tokio::task::spawn_blocking`, so SQLite never runs on a runtime worker.
- WAL, `foreign_keys = ON`, `busy_timeout = 5000`, `synchronous = NORMAL`.
- Versioned migrations (`PRAGMA user_version`), each applied in its own
  transaction with the version bump.
- Incremental writes: one `ExchangeUpdate` per processor output, one transaction
  each. Retention (`max_exchanges_per_session`) is swept periodically and the
  deletion is reported.
- Files are created with mode `0600` inside a `0700` directory.

### Guarantees and their limits

An exchange is durable once its `apply` transaction commits. A crash can lose
the event currently in flight and any event still buffered in the provider
channel; it cannot leave a half-written exchange, because headers, bodies and
row data are written in one transaction. Sessions left in `capturing`/`paused`
are recovered on the next start and marked `stopped` with a finish instant —
their stored exchanges are untouched.

## UI layer

`netra-tui` is state and rendering only: no clock, no database, no network. The
host feeds it a `Snapshot` (header + rows) and an `InspectorData` for the
selection, and receives `AppAction`s. That keeps the UI testable with
`TestBackend` and guarantees the UI cannot become the source of truth.

## Adding a capture provider (M3/M4)

Implement `CaptureProvider`:

1. Normalize native events into `CaptureEvent` (`RequestStarted`,
   `ResponseReceived`, `BodyUpdated`, `RequestCompleted`, `RequestFailed`,
   `RequestAborted`, `RequestRedirected`, `SourceDisconnected`/`Reconnected`).
2. Use a stable provider-local request id: it is the correlation key.
3. Leave limits, redaction and persistence to the pipeline — do not store or
   log payloads in the adapter.
4. Register the provider in `netra-cli`'s `providers_for` and delete the
   corresponding "not implemented" arm.
