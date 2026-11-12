# ADR 0002 — CDP and proxy are separate providers behind one port

## Context

Traffic can be observed through Chrome DevTools Protocol or through a local
proxy. The two differ in framing, identifiers, timing availability, body
availability and failure modes.

## Decision

Model both as implementations of `CaptureProvider`, emitting normalized
`CaptureEvent`s. Correlation, limits, redaction and persistence live in the
shared processor, not in the adapters.

## Consequences

- The domain and storage never mention CDP or proxy concepts.
- Deduplication of the same request seen through both sources is a processor
  policy decision; the processor understands correlation keys but never deletes
  records on its own.
- Each adapter is independently testable, and a future source (for example
  packet capture) plugs in without touching storage or the UI.
