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

- A credential in a **binary** body, or in a non-HTTP protocol captured as a
  body, is not recognised. Use `"capture_bodies": false` when capturing
  authentication flows you do not want on disk.
- Form-encoded bodies (`application/x-www-form-urlencoded`) are scanned like a
  query string. A **plain-text** body is never rewritten: masking free text means
  guessing where a URL ends, and a wrong guess either eats the prose or leaves the
  credential behind. When it names a configured sensitive parameter the payload
  is withheld and only its size is kept. A credential in an arbitrary text shape
  (a `token: abc` log line, a multipart part) is not recognised.
- A credential inside a non-standard header (`x-my-token`) is not recognised
  unless it is added to the rule list. The defaults cover `authorization`,
  `proxy-authorization`, `cookie`, `set-cookie`, `api-key`, `authentication`,
  `x-api-key`, `x-auth`, `x-auth-token`, `x-csrf-token` and `x-xsrf-token`.
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

## CDP capture (implemented)

Attaching to a browser gives `netra` access to everything that browser carries:
cookies, tokens and personal data. The adapter therefore:

- Connects only to an endpoint the user names (`--cdp-endpoint`, default
  `127.0.0.1:9222`) and never launches or configures a browser itself.
- Treats HTTP bodies above 8 MiB as omitted instead of materialising them.
- Applies the same redaction, limits and persistence path as every other source,
  so nothing captured over CDP bypasses the policy.

The debugging port itself is a privileged surface: any local process that can
reach it can drive the browser. `netra` does not open it — enabling
`--remote-debugging-port` is the user's decision, and it should be a separate
browser profile that is closed when the session ends. The adapter never asks for
`--remote-allow-origins=*`.

## Proxy and local authority (implemented)

- Listens on `127.0.0.1` by default. Binding anything else is refused unless
  `proxy.allow_remote` is `true`, and the adapter enforces that itself, not only
  the configuration validator.
- CONNECT without interception is a byte tunnel: it is reported as a tunnel and
  never inspected.
- TLS interception is opt-in (`proxy.intercept_tls`) **and** requires a CA
  created by `netra ca init`. Without material, capture fails closed and says so.
- The CA private key is written `0600` inside a `0700` directory, only once;
  `netra ca init` never replaces an existing authority. `netra ca show` prints the
  PEM's SHA-256 so it can be compared before trusting it, and `netra ca export`
  produces the file to import. Nothing is installed anywhere by `netra`.
- Upstream TLS is verified against the Mozilla roots plus `proxy.extra_trust_roots`.
  Certificate validation is never disabled, not even for testing.
- Limits: `proxy.max_connections` (64), `proxy.max_body_bytes` (8 MiB; larger
  bodies close the connection instead of being buffered), 64 KiB head limit, and
  `proxy.timeout_secs` (30) per step.
- Unsupported by design, and documented rather than worked around: certificate
  pinning, QUIC/HTTP-3, HTTP/2 upstreams, and clients that ignore proxy settings.

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
