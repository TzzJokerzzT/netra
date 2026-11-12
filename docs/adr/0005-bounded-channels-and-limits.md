# ADR 0005 — Bounded channels, explicit limits, no silent drops

## Context

Capture produces events faster than a terminal needs them, and a runaway
session can exhaust memory or disk. Dropping events silently would make the
inspector lie.

## Decision

- One bounded `mpsc` (1024) between providers and the consumer; providers await
  on a full channel instead of dropping.
- Payload, header and per-session limits in configuration, enforced in the
  processor and the retention sweep.
- Every drop, truncation, omission and failed write becomes a counter or a
  warning notification.

## Consequences

- Memory stays bounded regardless of traffic volume.
- A slow disk slows capture instead of corrupting it.
- Users can see how much was discarded, which is the difference between a tool
  that is incomplete and a tool that is wrong.
