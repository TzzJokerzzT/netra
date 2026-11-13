# Releasing netra

`netra` is distributed as a statically linked Linux x86_64 binary attached to a
GitHub Release on `github.com/TzzJokerzzT/netra`. A release is what `netra
update` offers and what `install.sh` installs.

## Versioning

- The single source of truth is `[workspace.package] version` in the root
  `Cargo.toml`. Every crate inherits it (`version.workspace = true`).
- A release is the tag `v<version>`, for example `v0.2.0`. The release workflow
  refuses to publish when the tag and the workspace version disagree, so a
  mismatched tag fails instead of shipping a binary that reports the wrong
  number.
- Versions follow semantic versioning for the CLI surface and the stored
  database:
  - **patch** — fixes with no command, output or schema change;
  - **minor** — new commands, flags, or a schema migration the tool applies on
    open;
  - **major** — a change that requires the user to act, such as removing a
    command or refusing to open an older database.
- The version shown by `netra --version` is the crate version plus the commit the
  binary was built from, `netra 0.2.0 (<git sha>)`. A local build prints
  `unknown` because no `NETRA_BUILD_GIT_SHA` was set at compile time.

## Cutting a release

1. Bump `[workspace.package] version` in `Cargo.toml` and run `cargo build
   --locked` so `Cargo.lock` follows.
2. Make sure `cargo test --workspace`, `cargo clippy --workspace --all-targets
   -- -D warnings` and `cargo fmt --all --check` pass locally. The workflow runs
   the tests again before it publishes, and stops there if they fail.
3. Commit, then tag the commit and push the tag:

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. The workflow `.github/workflows/release.yml` then:
   - refuses a tag that does not match the workspace version;
   - tests the workspace;
   - builds `netra-cli` for `x86_64-unknown-linux-musl` with `musl-gcc`, so the
     result carries no libc dependency;
   - strips the binary and packages it as `netra-x86_64-unknown-linux-musl`
     and `netra-x86_64-unknown-linux-musl.tar.gz`;
   - writes `sha256sum`-format checksums next to both;
   - runs the packaged binary and fails unless it reports both the tagged
     version and the commit;
   - publishes a GitHub Release with those four assets and generated notes.

Nothing else publishes: there is no crates.io upload and no other artifact
channel yet.

## What the update path does

`netra update` reads
`https://api.github.com/repos/TzzJokerzzT/netra/releases/latest`, compares the
tag with the running version, and when it is newer downloads the raw binary
asset plus its `.sha256`, verifies the digest, and replaces the running
executable. The replacement is staged in the same directory as the target and
renamed over it, so the previous binary stays intact until the new one is
complete and verified.

`--check` reports and downloads nothing. `--force` reinstalls the same version.

The startup notice is a separate, cheaper path: it reads a cached answer from
`$XDG_DATA_HOME/netra/update-check.json`, refreshes that cache in the background
at most once per 24 hours with a 2-second timeout, and prints one line to stderr
when a newer release is known. It never downloads a binary and never fails a
command. `{"update": {"check": false}}` in `config.json` disables both the
notice and the refresh.

## What the checksums do not protect against

The SHA-256 next to a release proves the bytes were not corrupted in transit and
not truncated. It does **not** prove authenticity: it is published from the same
GitHub account that published the binary, so anyone able to publish a release
can publish a matching checksum. Today the trust anchor is the repository
account itself.

Consequences worth knowing:

- Treat a `netra` binary from anywhere other than the release page as untrusted,
  the same way you would treat any other program.
- `netra update` never runs on its own. Nothing replaces a binary without the
  user typing `netra update` (or running `install.sh`).
- A signature over the assets (for example minisign or cosign with a key kept
  off GitHub) is the natural next step if a second distribution channel appears
  — see ADR 0008.

## Manual fallback

```bash
# install a specific tag without the script
tag=v0.2.0
base=https://github.com/TzzJokerzzT/netra/releases/download/$tag
curl -fsSLO "$base/netra-x86_64-unknown-linux-musl"
curl -fsSLO "$base/netra-x86_64-unknown-linux-musl.sha256"
sha256sum -c netra-x86_64-unknown-linux-musl.sha256
install -Dm755 netra-x86_64-unknown-linux-musl ~/.local/bin/netra
```

If GitHub is unreachable, `./install.sh --from-source` builds the checkout with
cargo and installs the same way.
