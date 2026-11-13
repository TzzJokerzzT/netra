# netra — Release versioning, self-update and installer

Feature branch: `feat/netra-foundation`. Release channel: `github.com/TzzJokerzzT/netra`.
Document scope: turning the project into a versioned, self-updating, one-command-installable app.

## Specs

- **S1 (versionamiento).** "Ahora necesito que crees versionamiento para cuando se realice un release la app se
  actualice." A release is a git tag `vX.Y.Z` that matches `[workspace.package] version`; publishing it must be
  what makes an update available to an installed `netra`.
- **S2 (instalador).** "Tambien que generes un script para cuando el usuario instale haga todo el proceso de
  instalación." The script must be the whole installation: obtain the binary, verify it, put it in a directory
  that is in `PATH`, and prove the result runs.
- **S3 (README).** "y agregues eso al readme." The README install and update sections must describe the script
  and `netra update`, and the release process must be documented for the maintainer.
- **S4 (mecanismo de actualización — decisión del usuario).** "netra update + aviso al arrancar" —
  "Comando explícito que descarga, verifica sha256 y reemplaza el binario; al arrancar solo muestra un aviso si
  hay versión nueva (desactivable en config)." Accepted user-visible contract:
  "netra 0.1.0 → 0.2.0 available" · "sha256: verificado contra el .sha256 del release" ·
  "installed: /home/you/.local/bin/netra (replacing 0.1.0)" · "⚠ netra 0.2.0 disponible — corré `netra update`".
- **S5 (plataforma publicada — decisión del usuario).** "Solo Linux x86_64 (musl estático)" —
  "Un asset, sin dependencias de libc en el destino."
- **S6 (límites que el producto declara).** The update path verifies a SHA-256 published next to the same
  release; that detects corruption and truncation, not a compromised release. The docs state this limit instead
  of implying the binary is signed. `netra update` never runs implicitly from a command the user did not ask for,
  and the startup notice never blocks, never writes into the TUI, and never downloads anything.

## Tasks

| ID | Specs | Route | What | Commit |
| --- | --- | --- | --- | --- |
| T1 | S1 | inline | Version source of truth: `repository` points at the real remote, and the release job refuses a tag that disagrees with `[workspace.package] version` | — |
| T2 | S1, S5 | inline | `.github/workflows/release.yml`: on `v*` tag build `x86_64-unknown-linux-musl`, strip, package `.tar.gz` + `.sha256`, publish a GitHub Release | — |
| T3 | S4, S6 | worker | `netra update` in `netra-cli`: latest-release query over HTTPS, semver compare, `--check`/`--force`, download, sha256 verification, atomic binary replacement, and `netra --version` reporting the build it came from | — |
| T4 | S4, S6 | worker | Startup notice: cached latest-version check (bounded, non-blocking, opt-out via config), one stderr line before the TUI | — |
| T5 | S2, S5 | inline | `install.sh`: platform check, release download + sha256, install into `${NETRA_INSTALL_DIR:-$HOME/.local/bin}`, `PATH` diagnosis, version proof, `--from-source` fallback while no release exists | — |
| T6 | S3, S6 | inline | README install/update/release sections, `docs/release.md`, ADR 0008 for the update mechanism and its trust limit | — |
| T7 | S1-S6 | inline | Checks: workspace tests, clippy `-D warnings`, fmt, script dry run | — |

## Log

- **L1 (request, verbatim).** "Ahora necesito que crees versionamiento para cuando se realice un release la app
  se actualice. Tambien que generes un script para cuando el usuario instale haga todo el proceso de instalación
  y agregues eso al readme"
- **L2 (decision, verbatim).** Update mechanism: "netra update + aviso al arrancar". Published platform:
  "Solo Linux x86_64 (musl estático)".
- **L3 (evidence, checks).** `cargo test --workspace` 330 passed / 0 failed; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo fmt --all --check` clean; `bash -n install.sh` clean;
  `release.yml` parses as YAML and its version guard, packaging and `sha256sum -c` steps were run locally.
  Smoke tests: `install.sh` happy path, checksum mismatch (nothing installed, exit 1), non-executable asset
  (exit 1 naming the path), `PATH` hint; `netra update` end to end against a local fixture (download, sha256,
  atomic replace, contract lines) and a 404 API ("no release has been published", exit 1); startup notice seen
  on stderr and suppressed by `{"update": {"check": false}}`.
- **L4 (review, native).** Lineage `review-3302dabc1b765ce1`, four lenses over 2 870 changed lines. Four
  CRITICAL findings, all candidate-caused and deterministic: `R1-001` (`--check --force` silently ignored
  `--force`), `R3-INSTALL-PROOF` and `R4-002` (the installer's version probe swallowed a failure and reported
  success), `R4-001` (`netra config` gained an undocumented line). One bounded correction, 39 diff lines.
  The targeted validator accepted all four fixes (`original_criteria.passed: true`) and raised three
  regression claims; the lineage therefore closed `escalated`, and its authority is spent.
- **L5 (decision after the review).** Accepted `install.sh:154` (version probe captured with `2>&1`, so any
  stderr warning from a healthy binary would fail the install) and fixed it: only stdout is read now, verified
  with a fixture that writes to stderr and a non-executable asset that must fail. Accepted the no-rollback
  claim as documented behaviour: the binary stays in place and the error names the path. Rejected the
  `--check`/`--force` claim: an explicit usage error is the honest fix for a flag that used to be silently
  ignored, and the alternatives reintroduce the same silent ignore in the other direction.
