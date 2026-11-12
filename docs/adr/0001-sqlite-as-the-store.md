# ADR 0001 — SQLite as the local store

## Context

Sessions hold many small records (one exchange per request, with headers,
bodies and timings) that must be written incrementally, queried with filters and
pagination, and recovered after an unexpected exit. The tool is single-user and
local.

## Decision

Use SQLite through `rusqlite` (bundled), with WAL, foreign keys and versioned
migrations.

## Consequences

- Zero-install, single-file database with real transactions and indexes.
- SQL filtering and paging keep the TUI responsive on large sessions.
- `rusqlite` is synchronous, so every query runs in `spawn_blocking` behind one
  connection; throughput is bounded by one writer, which is far above what a
  single browser or proxy produces.
- Alternatives (embedded key-value stores, a custom log format) would need
  hand-written indexing, paging and integrity logic: more code and more ways to
  corrupt data.
