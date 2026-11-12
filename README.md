# netra — network inspector for the terminal

`netra` captures, inspects, filters and persists HTTP traffic from the terminal,
in the spirit of Chrome DevTools' Network panel, `mitmproxy`, `k9s` and `lazygit`.

The project is built in milestones. **What exists today is a working
foundation**: a modular Rust workspace, a real capture pipeline with
correlation and redaction, SQLite persistence with crash recovery, and a TUI
that browses and inspects stored sessions. Chrome DevTools Protocol capture and
the HTTP/HTTPS proxy are the next milestones — they are not implemented yet, and
the CLI says so instead of pretending.

## Status

| Milestone | Scope | State |
| --- | --- | --- |
| M0 | Workspace, domain, ports, CLI, CI | done |
| M1 | Ratatui/crossterm TUI: layout, list, inspector tabs, filters, help | done |
| M2 | SQLite: migrations, incremental persistence, recovery, limits | done |
| M3 | CDP capture (Chrome/Chromium) | done |
| M4 | HTTP/HTTPS proxy with local CA | not started |
| M5 | Analysis depth (waterfall UI, search UX) | partially: analysis engine done |
| M6 | Replay, HAR import, compare, WebSockets, stats UI | not started |
| M7 | Hardening and release | not started |

`netra capture --demo` runs a **synthetic** scripted capture. It exercises the
whole pipeline (correlation, redaction, limits, storage, recovery) and is
labelled as demo everywhere it appears. It is not real traffic capture.

## Requirements

- Rust stable (edition 2024, `rust-version = 1.85` in `Cargo.toml`).
- A POSIX system for the private-permission and terminal behaviour described
  here. Linux is the tested target.
- SQLite is compiled in (`rusqlite` with the `bundled` feature); no system
  package is required.

## Build and test

```bash
cargo build --release            # binary at target/release/netra
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## Usage

```bash
netra                              # TUI over the most recent session
netra capture --demo --source cdp  # scripted synthetic capture (~1s)
netra capture --source cdp         # live capture from Chrome/Chromium
netra capture --source cdp --cdp-endpoint 127.0.0.1:9333
netra sessions list                # table; --json for machine output
netra sessions open <id>           # TUI for one session (unique id prefix works)
netra sessions rename <id> <name>
netra sessions save <id>           # records an explicit save instant
netra sessions export <id> --format har --output ./traffic.har
netra sessions delete <id> --yes   # deleting is always explicit
netra config                       # paths, limits, proxy and redaction summary
netra config --json
```

### Keyboard (TUI)

`j`/`k` move, `g`/`G` first/last, `Enter` focus the inspector, `Tab`/`Shift-Tab`
cycle panes and tabs, `1`–`7` select an inspector tab, `/` search, `f` filters,
`F` only-failed, `s` save, `p` pause/resume, `c` clear the view, `r` replay
(confirmation), `d` delete (confirmation), `e` export HAR, `?` help, `q` quit.

`c` only clears filters, selection and the current tab. It never deletes stored
records: capture data lives in SQLite, not in the UI.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | runtime failure (I/O, storage, capture) |
| 2 | usage error (clap) |
| 3 | feature not available yet |
| 4 | requested session does not exist |

### Live CDP capture

Start a browser with the DevTools port open, in its own profile, then capture:

```bash
chromium --remote-debugging-port=9222 --user-data-dir=/tmp/netra-profile &
netra capture --source cdp --name "checkout flow"
# browse in that browser; Ctrl-C stops capture and persists the session
```

`netra` connects to an already-running browser; it never launches one and never
changes its flags. Default endpoint is `127.0.0.1:9222`.

Known limits of CDP capture (documented in `docs/architecture.md`): response
headers collapsed by the protocol stay collapsed, WebSockets are not inspected
yet, `responseReceivedExtraInfo` is not consumed, and payloads over 8 MiB are
reported as omitted instead of being loaded.

## Where data lives

- Config: `$XDG_DATA_HOME/netra/config.json` (default
  `~/.local/share/netra/config.json`).
- Database: `$XDG_DATA_HOME/netra/netra.db`, created with mode `0600` inside a
  `0700` directory; the WAL sidecars are tightened to `0600` as soon as the
  journal mode is enabled.
- A `config.json` may set `data_dir`; it is honoured (relative paths resolve
  against the configuration file) and never written back by the tool.
- Exports: `$XDG_DATA_HOME/netra/exports/`.

## What is persisted, and what is not

Every captured exchange is written incrementally while capture runs, so a crash
loses at most the event in flight. Credentials are masked **before** storage:

- `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`,
  `x-auth-token`, `x-csrf-token`, `x-xsrf-token` become `[REDACTED]`.
- Query parameters such as `token`, `api_key`, `password`, `signature` are
  masked in the stored URL.
- JSON fields such as `password`, `token`, `client_secret` are masked in stored
  payloads.
- Payloads above the configured limit are truncated; **structured payloads
  (JSON/XML) above the limit are omitted entirely** rather than stored as a
  prefix that redaction can no longer inspect.

See `docs/security.md` for the threat model and the known limits.

## Documentation

- `docs/architecture.md` — layers, crates, contracts and data flow.
- `docs/security.md` — threat model, redaction, proxy and replay rules.
- `docs/roadmap.md` — milestones and what each one still needs.
- `docs/adr/` — architecture decision records.

## License

MIT OR Apache-2.0.
