# ADR 0004 — Redact in the pipeline, before storage

## Context

Captured traffic routinely contains bearer tokens, cookies and passwords.
Redacting at render time or at export time leaves them on disk.

## Decision

Apply the redaction policy in the capture processor, before the first write, and
keep it applied for every later read and export. Body capture is a policy switch
(`capture_bodies`), and structured payloads above the size limit are omitted
instead of truncated, because a truncated JSON document can no longer be
inspected by the JSON redactor.

## Consequences

- Nothing that the policy recognises is ever written to disk.
- Evidence that a header was present remains (`Authorization: [REDACTED]`),
  which is what an inspector needs.
- Unrecognised shapes (custom headers, form bodies) are not masked; this is
  documented in `docs/security.md` together with the mitigation.
- Redaction is not reversible, by design.
