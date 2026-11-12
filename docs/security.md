# Security model

`netra` handles other people's credentials. This document states what the tool
does, what it does **not** do, and where the residual risk is.

## Threat model

Assets:

1. Credentials and session tokens that appear in captured traffic.
2. Captured payloads (personal data, business data).
3. The local certificate authority private key (once TLS interception exists, milestone M4).
4. The integrity of the stored history.

Adversaries considered:

- Another local user or process reading `netra`'s files.
- Someone who gains access to an exported HAR file.
- A hostile or accidental configuration that turns `netra` into an open proxy
  or a local SSRF pivot.
- A crashed or killed process leaving data in an inconsistent state.

Not considered: a local attacker with the same user privileges who can attach a
debugger to the running process, or full-disk encryption attacks.

## Redaction (implemented, default on)

Applied by the capture processor **before** anything is persisted or exported:

| Surface | Default rules |
| --- | --- |
| Headers | `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, `x-auth-token`, `x-csrf-token`, `x-xsrf-token` become `[REDACTED]` |
| Query strings | `access_token`, `api_key`, `apikey`, `auth`, `code`, `id_token`, `key`, `password`, `refresh_token`, `sig`, `signature`, `token` |
| JSON fields | `access_token`, `api_key`, `apikey`, `authorization`, `client_secret`, `id_token`, `password`, `passwd`, `refresh_token`, `token` |
| Payloads | stored only up to `limits.max_body_bytes`; JSON/XML above the limit are omitted, never stored truncated |

Rules are configurable in `config.json` (`redaction.header_names`,
`redaction.query_params`, `redaction.json_fields`, `redaction.capture_bodies`).
Configuration **adds** rules to the built-in carriers; it never removes them, so
listing one extra header cannot silently un-protect `Authorization` or `Cookie`.
Redaction preserves header names, order and duplicates so the evidence that a
header was present is not lost.

A captured URL that no parser accepts (`https://host:99999/x?token=...`,
`data:`, malformed redirects) is masked textually: the parse failure is never a
redaction bypass.

Redaction runs twice for safety: once when the response/request object is built
and once on the whole exchange before it is handed to storage. Exports reuse the
stored, already-redacted rows.

### Known limits of redaction

- A credential in a **non-JSON** body (form-encoded login, custom protocol,
  binary) is not recognised. Use `"capture_bodies": false` when capturing
  authentication flows you do not want on disk.
- A credential inside a non-standard header (`x-my-token`) is not recognised
  unless it is added to the rule list.
- A malformed JSON body is not rewritten: it is stored as captured, because
  rewriting an unparsable document is not safe.
- Truncation of text/binary payloads can cut a value in half; the remaining
  prefix is still on disk.
- Redaction cannot protect a HAR file you export and then share. Exports keep
  the redaction applied at capture time, but nothing else.

## Storage (implemented)

- The data directory is created `0700` and a pre-existing one is tightened to
  `0700` on the next open.
- The database file is created with `0600` **before** SQLite opens it, so there
  is no window in which another local process could open it with the process
  umask. The `-wal` and `-shm` sidecars are tightened immediately after the
  journal mode is enabled.
- HAR exports are created `0600` (`--output` included), because an export holds
  whatever redaction could not recognise.
- SQLite runs with WAL, foreign keys and a busy timeout.
- Diagnostic logging never includes payloads, headers or query values.
- `PRAGMA synchronous = NORMAL`: a power loss can lose the last transactions;
  the database stays consistent.
- A live session records the pid of the process capturing it, so a second
  `netra` command never recovers (and stops) a capture that is still running.

## Proxy (milestone M4, not implemented)

The contract the implementation must honour:

- Listen on `127.0.0.1` by default; binding elsewhere requires an explicit
  configuration change.
- Never accept unauthenticated external connections: an open proxy is a data
  exfiltration path.
- Limit concurrent connections (`proxy.max_connections`), header size and body
  size, and set timeouts.
- Support CONNECT tunnelling **without** decryption, and make TLS interception a
  deliberate per-session action.
- A local certificate authority is generated only on explicit user action, its
  private key is written with `0600`, and nothing is installed into the system
  trust store automatically. Trust and removal instructions ship with the
  feature, not before it.
- Certificate pinning, QUIC/HTTP-3 and clients that ignore proxy settings are
  documented as unsupported rather than worked around.

## Replay and SSRF (milestone M6, not implemented)

Rules the future implementation must follow:

- Never replay automatically; always require confirmation.
- Show method, destination and a warning before sending.
- Block or require reinforced confirmation for loopback, private ranges,
  link-local and cloud metadata endpoints.
- Never forward credentials to a different host than the captured one.
- Validate redirect targets against the same policy.
- Keep an allowlist/denylist in configuration.

Until then the TUI's `r` key opens a confirmation dialog and answers that replay
is not implemented; nothing is ever sent.

## Limits and retention (implemented)

- `limits.max_body_bytes` (default 1 MiB) caps stored payloads.
- `limits.max_headers_per_side` (default 200) caps header lines.
- `limits.max_exchanges_per_session` (default 50 000) triggers retention sweeps;
  deletions are reported to the user.
- Orphan responses are cached in a bounded map (256 entries).

## Reporting a problem

Security issues should be reported privately to the maintainers rather than as a
public issue with reproduction data attached.
