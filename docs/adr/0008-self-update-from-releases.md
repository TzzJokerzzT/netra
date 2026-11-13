# ADR 0008 — Self-update from GitHub Releases with checksum verification

Status: accepted (milestone M7).

## Context

`netra` is installed as a prebuilt binary, not through a package manager, so the
only way a user learns about a fix is by looking. That is a bad default for a
tool that reads traffic: a stale capture adapter misses protocol changes and a
stale redaction rule is a leak.

Replacing the running binary is also the most privileged thing the tool can do.
`netra` runs with the user's own access, holds a local certificate authority and
is trusted with traffic that may contain credentials. An update path that
downloads and executes content from the network deserves an explicit design, not
a convenience shortcut.

Constraints already in the project: no new HTTP client stack (ADRs 0006 and 0007
chose in-tree clients over embedding large crates), one tested platform (Linux
x86_64), and a strong preference for behaviour that is provable with tests
rather than behaviour that only works when the network is healthy.

## Options considered

| Option | Assessment |
| --- | --- |
| No self-update, document the manual download | Cheapest, but the user only updates when they remember; the README already showed it and updates did not happen. |
| Delegate to a package manager (crates.io, AUR, distro package) | The right long-term answer for system integration, but it needs a publisher and packaging for each channel, and it does not help anyone today. |
| Self-update from GitHub Releases with a published SHA-256 | Small, testable, no new dependency, and it closes the "user never updates" gap; the checksum detects corruption, not a compromised release. |
| Self-update with signed assets (minisign/cosign) | Stronger, and the natural follow-up, but it adds key management and a second trust root before there is a second channel that needs it. |

## Decision

Ship the third option, and be explicit about what it does not cover.

- **A release is a tag.** `vX.Y.Z` must match `[workspace.package] version`;
  the release workflow refuses the mismatch, so "what is published" and "what
  the binary reports" cannot drift.
- **The binary is static.** `x86_64-unknown-linux-musl`, stripped, published
  with a `.sha256` next to both the raw binary and the tarball. One asset, one
  platform, no libc coupling — the same choice as using SQLite bundled.
- **Updating is a command, never a background mutation.** `netra update` is the
  only path that downloads a binary. It verifies the digest, stages the new file
  in the target directory, and renames it over the old one so the previous
  binary survives an interrupted update.
- **The startup notice is passive.** It reads a cached answer, refreshes it at
  most once per 24 hours behind a 2-second timeout in a detached task, prints at
  most one stderr line before the TUI takes the terminal, never downloads, and
  never fails a command. `{"update": {"check": false}}` turns it off.
- **The HTTP client stays in-tree.** The release query reuses the rustls stack
  the proxy already depends on and hand-rolls the HTTP/1.1 request, following
  ADR 0006 and ADR 0007 rather than adding a client crate that pulls its own TLS
  and redirect policy.

## Consequences

- GitHub is the delivery channel. If it is down, throttled, or the account is
  unavailable, both the installer and `netra update` fail; `install.sh
  --from-source` remains the escape hatch and the docs say so.
- The trust anchor is the GitHub account, not cryptography over the release. A
  compromised account can ship a binary with a matching checksum. This is stated
  in `docs/release.md` and in the README instead of being implied away.
- Only Linux x86_64 is published. The installer refuses other platforms rather
  than installing a binary that cannot run; macOS and arm64 need their own
  targets and their own testing before they claim support.
- The update path cannot be exercised until the first release exists, so its
  pure parts (version comparison, release parsing, checksum verification, atomic
  replacement, the loopback-only override) are unit-tested and the network layer
  stays deliberately thin.
- Adding a signature later is a contained change: the same plan-and-verify seam
  gains one more verification step before the rename.
