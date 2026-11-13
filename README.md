# netra — network inspector for the terminal

`netra` captures, inspects, filters and persists HTTP traffic from the terminal,
in the spirit of Chrome DevTools' Network panel, `mitmproxy`, `k9s` and `lazygit`.

The project is built in milestones. Milestones **M0 to M4 are implemented and
tested**: a modular Rust workspace, a real capture pipeline with correlation and
redaction, SQLite persistence with crash recovery, a TUI that browses and
inspects stored sessions, live capture from Chrome/Chromium over CDP, and an
HTTP/HTTPS proxy with an explicit local certificate authority. Replay, HAR
import and WebSocket inspection are still open, and the CLI says so instead of
pretending.

## Status

| Milestone | Scope | State |
| --- | --- | --- |
| M0 | Workspace, domain, ports, CLI, CI | done |
| M1 | Ratatui/crossterm TUI: layout, list, inspector tabs, filters, help | done |
| M2 | SQLite: migrations, incremental persistence, recovery, limits | done |
| M3 | CDP capture (Chrome/Chromium) | done |
| M4 | HTTP/HTTPS proxy with local CA | done |
| M5 | Analysis depth (waterfall UI, search UX) | partially: analysis engine done |
| M6 | Replay, HAR import, compare, WebSockets, stats UI | not started |
| M7 | Hardening and release | partially: versioning, release automation and self-update |

`netra capture --demo` runs a **synthetic** scripted capture. It exercises the
whole pipeline (correlation, redaction, limits, storage, recovery) and is
labelled as demo everywhere it appears. It is not real traffic capture.

## Install

`netra` ships as a statically linked Linux x86_64 binary through GitHub
Releases, so installing a release needs no toolchain. It is not on crates.io
yet.

The install script does the whole job: it takes the latest release, verifies its
SHA-256, installs the binary into `~/.local/bin` (or the directory you name),
tells you if that directory is not in `PATH`, and proves the result runs.

```bash
curl -fsSL https://raw.githubusercontent.com/TzzJokerzzT/netra/main/install.sh | bash
```

Read it before handing it a shell — that is the point of a script that verifies
checksums:

```bash
curl -fsSL -O https://raw.githubusercontent.com/TzzJokerzzT/netra/main/install.sh
less install.sh
bash install.sh
```

Options:

```bash
./install.sh --dir ~/bin         # install somewhere else
./install.sh --version v0.2.0    # pin a release instead of taking the latest
./install.sh --from-source       # build this checkout with cargo
```

`NETRA_INSTALL_DIR`, `NETRA_VERSION` and `NETRA_REPO` do the same from the
environment. The script refuses a platform other than Linux x86_64 instead of
installing a binary that cannot run, refuses a release whose checksum does not
match what it downloaded, and exits non-zero naming the path when the installed
binary cannot execute (a `noexec` mount, a lost execute bit) — the binary stays
in place so you can fix the cause instead of reinstalling.

### From a checkout, with cargo

Use this when no release exists yet, or when you want your own build:

```bash
cargo install --path crates/netra-cli --root ~/.local --locked   # ~/.local/bin/netra
cargo install --path crates/netra-cli --locked                   # $CARGO_HOME/bin
```

If `netra: command not found` appears afterwards, that directory is not in
`PATH`:

```bash
export PATH="$HOME/.local/bin:$PATH"   # add to ~/.zshrc / ~/.bashrc / config.fish
```

### Portable binary build

No runtime dependencies are needed beyond libc: SQLite is compiled in.

```bash
cargo build --release
install -Dm755 target/release/netra ~/.local/bin/netra
```

### Verify the installation

```bash
netra --version
netra config          # shows the data directory, limits, proxy, redaction and update check
netra capture --demo  # synthetic capture, ~1s, proves storage + redaction work
netra sessions list
netra                 # TUI over the most recent session (q to quit)
```

Then start the browser source you want:

```bash
# CDP
chromium --remote-debugging-port=9222 --user-data-dir=/tmp/netra-profile &
netra capture --source cdp

# proxy
netra capture --source proxy --proxy-port 8080
```

### Update and uninstall

An installed `netra` updates itself from the latest published release:

```bash
netra update --check    # what is published versus what you run; downloads nothing
netra update            # download, verify the SHA-256, replace the binary
netra update --force    # reinstall even when the version is already current
```

The replacement is atomic: the new binary is staged next to the target and
renamed into place, so an interrupted update leaves the previous binary intact.
`netra update` refuses a checksum mismatch and never writes into a directory you
cannot write to — it names the path instead. `--check` and `--force` are
mutually exclusive, and `--check` exits non-zero when there is no release to
compare against (none published, or the API is unreachable) instead of claiming
the build is current. The startup notice only says that a
newer release exists; it never downloads anything by itself. Turn it off in
`$XDG_DATA_HOME/netra/config.json` (default `~/.local/share/netra/config.json`):

```json
{ "update": { "check": false } }
```

From a checkout, update the checkout instead: `git pull && ./install.sh
--from-source`.

Uninstall by removing the binary (`rm "$(command -v netra)"`, or
`cargo uninstall netra-cli` for a cargo install). Captured data stays behind by
design: it lives in `$XDG_DATA_HOME/netra` (default `~/.local/share/netra`).
Remove that directory only if you also want to delete the sessions, the exported
HAR files and the local certificate authority.

## Requirements

- Rust stable (edition 2024, `rust-version = 1.85` in `Cargo.toml`) to build
  from source. The installed binary needs no toolchain.
- A POSIX system for the private-permission and terminal behaviour described
  here. Linux is the tested target.
- SQLite is compiled in (`rusqlite` with the `bundled` feature); no system
  package is required.

## Build and test

```bash
cargo build --release            # binary at target/release/netra
cargo build --release -p netra-cli --target x86_64-unknown-linux-musl   # what a release ships
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## Release

A release is a tag, `vX.Y.Z`, whose version matches `[workspace.package]
version` in `Cargo.toml`. Pushing it runs `.github/workflows/release.yml`, which
tests the workspace, builds the static `x86_64-unknown-linux-musl` binary,
publishes it with a `.sha256` alongside, and is what makes `netra update` offer
the new build. The full procedure, including how the version is chosen and what
the checksums do and do not protect, is in `docs/release.md`.

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

### Proxy capture

```bash
netra capture --source proxy --proxy-port 8080
# point a browser or a tool at 127.0.0.1:8080
```

Plain HTTP is captured immediately. HTTPS is captured either as an opaque CONNECT
tunnel (default) or decrypted when interception is enabled:

```bash
netra ca init                      # create the local authority once
netra ca export --output netra-ca.pem
# import that certificate in your browser/OS trust store, then:
# set "intercept_tls": true under "proxy" in config.json
netra ca show                      # paths and the certificate's SHA-256
```

`netra` never installs certificates. Remove the trusted entry to undo
interception.

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
- `docs/release.md` — versioning, cutting a release, and how self-update works.
- `docs/adr/` — architecture decision records.

## License

MIT OR Apache-2.0.
