# ADR 0003 — Persist incrementally, per event

## Context

The brief requires that a crash never loses confirmed records, that refreshing
or navigating the UI never destroys data, and that a session can be recovered
after an unexpected exit.

## Decision

Every processed event produces one `ExchangeUpdate` applied in one transaction.
"Saving" a session only records an explicit `saved_at` instant; it is not the
moment data reaches disk.

## Consequences

- A crash loses at most the event in flight.
- The UI is a pure reader, so it can never be the source of truth.
- More write transactions than a batching approach; SQLite in WAL mode handles
  this comfortably, and the retention sweep keeps the database bounded.
- `apply` must stay idempotent per update kind, because providers replay events
  and late responses legitimately override earlier ones.
