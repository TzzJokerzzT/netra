#!/usr/bin/env bash
# netra installer.
#
# Downloads the latest published netra release for Linux x86_64, verifies it
# against the SHA-256 published with that release, installs it into a directory
# that is already in PATH, and proves the result runs.
#
#   curl -fsSL https://raw.githubusercontent.com/TzzJokerzzT/netra/main/install.sh | bash
#   ./install.sh --version v0.2.0 --dir ~/bin
#   ./install.sh --from-source        # build this checkout instead of downloading
#
# Environment:
#   NETRA_INSTALL_DIR   target directory (default: $HOME/.local/bin)
#   NETRA_VERSION       release tag to install, for example v0.2.0
#   NETRA_REPO          GitHub repository, default TzzJokerzzT/netra
set -euo pipefail

REPO="${NETRA_REPO:-TzzJokerzzT/netra}"
ASSET="netra-x86_64-unknown-linux-musl"
BINARY="netra"

TARGET_DIR="${NETRA_INSTALL_DIR:-${HOME:-}/.local/bin}"
TAG="${NETRA_VERSION:-}"
FROM_SOURCE=0
# Version string proven by the installed binary, set by `prove_install`.
PROVEN=""

say() { printf 'netra-install: %s\n' "$*"; }
die() { printf 'netra-install: error: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'USAGE'
Install netra from its latest GitHub release, or build this checkout.

Usage: install.sh [options]

Options:
  --dir <path>        install into <path> (default: $HOME/.local/bin)
  --version <tag>     install a specific release, for example v0.2.0
  --from-source       build this checkout with cargo instead of downloading
  -h, --help          show this help

Environment:
  NETRA_INSTALL_DIR   same as --dir
  NETRA_VERSION       same as --version
  NETRA_REPO          GitHub repository, default TzzJokerzzT/netra
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dir)
      [ "$#" -ge 2 ] || die "--dir needs a path"
      TARGET_DIR="$2"
      shift 2
      ;;
    --version)
      [ "$#" -ge 2 ] || die "--version needs a tag such as v0.2.0"
      TAG="$2"
      shift 2
      ;;
    --from-source)
      FROM_SOURCE=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1 (try --help)"
      ;;
  esac
done

[ -n "${HOME:-}" ] || die "HOME is not set; pass --dir explicitly"
[ -n "$TARGET_DIR" ] || die "no install directory; pass --dir or set NETRA_INSTALL_DIR"

platform() {
  [ "$(uname -s)" = "Linux" ] ||
    die "$(uname -s) is not supported: releases are built for Linux x86_64 only"
  [ "$(uname -m)" = "x86_64" ] ||
    die "$(uname -m) is not supported: releases are built for x86_64 only (got $(uname -m))"
}

fetcher() {
  if command -v curl >/dev/null 2>&1; then
    printf 'curl'
  elif command -v wget >/dev/null 2>&1; then
    printf 'wget'
  else
    die "need curl or wget to download the release"
  fi
}

fetch_bytes() {
  local url="$1"
  case "$FETCH" in
    curl) curl -fsSL --proto '=https' --tlsv1.2 --max-time 120 "$url" ;;
    wget) wget -q -O - --https-only --timeout=120 "$url" ;;
    *) die "no downloader available" ;;
  esac
}

download_to() {
  local url="$1" destination="$2"
  case "$FETCH" in
    curl) curl -fsSL --proto '=https' --tlsv1.2 --max-time 600 -o "$destination" "$url" ;;
    wget) wget -q -O "$destination" --https-only --timeout=600 "$url" ;;
    *) die "no downloader available" ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    die "need sha256sum or shasum to verify the release"
  fi
}

latest_tag() {
  local payload tag
  payload="$(fetch_bytes "https://api.github.com/repos/${REPO}/releases/latest")" || {
    die "cannot reach the GitHub API for ${REPO}; check the network or pass --version"
  }
  tag="$(printf '%s' "$payload" |
    grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' |
    head -n 1 |
    sed 's/.*"\([^"]*\)"$/\1/')"
  [ -n "$tag" ] || die "no published release of ${REPO} yet; try --from-source in a checkout"
  printf '%s' "$tag"
}

[ "$FROM_SOURCE" -eq 1 ] || platform
FETCH="$(fetcher)"

install_from_source() {
  command -v cargo >/dev/null 2>&1 || die "--from-source needs cargo on PATH"
  [ -f "Cargo.toml" ] && [ -d "crates/netra-cli" ] ||
    die "--from-source must run from a netra checkout"

  say "building this checkout in release mode"
  cargo build --release --locked -p netra-cli
  [ -x "target/release/${BINARY}" ] || die "the build produced no target/release/${BINARY}"
  install_binary "target/release/${BINARY}"
  prove_install
  say "installed ${PROVEN} from source"
}

# Runs the installed binary and dies unless it reports a netra version. Without
# this the installer would report success for a binary the target cannot execute
# (a noexec mount, a stripped execute bit, a truncated asset).
#
# Only stdout is read: a healthy binary may warn on stderr, and that must not be
# mistaken for a broken version string.
prove_install() {
  local reported
  if ! reported="$("$TARGET_DIR/${BINARY}" --version)"; then
    die "the installed ${TARGET_DIR}/${BINARY} cannot run; the binary is in place, so fix the cause (a noexec mount, an execute bit) and run \`${TARGET_DIR}/${BINARY} --version\`"
  fi
  case "$reported" in
    netra*) PROVEN="$reported" ;;
    *) die "the installed binary reported an unexpected version: ${reported}" ;;
  esac
}

install_binary() {
  local source="$1" staged
  mkdir -p "$TARGET_DIR" 2>/dev/null || die "cannot create ${TARGET_DIR}"
  [ -w "$TARGET_DIR" ] || die "${TARGET_DIR} is not writable; pass --dir with a directory you own"

  # Stage next to the target and rename: `install`/`cp` would truncate a binary
  # that may be running right now (ETXTBSY), while rename is atomic.
  staged="${TARGET_DIR}/.${BINARY}.new.$$"
  install -m 0755 "$source" "$staged" 2>/dev/null || {
    rm -f "$staged"
    die "cannot write ${staged}"
  }
  mv -f "$staged" "${TARGET_DIR}/${BINARY}" || {
    rm -f "$staged"
    die "cannot replace ${TARGET_DIR}/${BINARY}"
  }
}

path_hint() {
  case ":${PATH}:" in
    *":${TARGET_DIR}:"*) return 0 ;;
    *) return 1 ;;
  esac
}

report_path() {
  if path_hint; then
    say "${TARGET_DIR} is in PATH"
    return 0
  fi
  say "${TARGET_DIR} is NOT in PATH; add it to your shell configuration:"
  printf '\n  export PATH="%s:$PATH"\n\n' "$TARGET_DIR"
  say "then restart the shell, or run ${TARGET_DIR}/${BINARY} directly"
}

install_from_release() {
  [ -n "$TAG" ] || TAG="$(latest_tag)"
  case "$TAG" in
    v*) ;;
    *) TAG="v${TAG}" ;;
  esac

  local base tmp expected actual
  base="https://github.com/${REPO}/releases/download/${TAG}"
  tmp="$(mktemp -d)" || die "cannot create a temporary directory"
  trap 'rm -rf "${tmp:-}"' EXIT

  say "downloading ${ASSET} from release ${TAG}"
  download_to "${base}/${ASSET}" "${tmp}/${ASSET}" ||
    die "cannot download ${base}/${ASSET}"
  download_to "${base}/${ASSET}.sha256" "${tmp}/${ASSET}.sha256" ||
    die "cannot download the checksum ${base}/${ASSET}.sha256"

  [ -s "${tmp}/${ASSET}" ] || die "the downloaded binary is empty"
  [ -s "${tmp}/${ASSET}.sha256" ] || die "the downloaded checksum is empty"

  expected="$(awk '{print $1}' "${tmp}/${ASSET}.sha256")"
  [ -n "$expected" ] || die "the published checksum file is empty"
  actual="$(sha256_of "${tmp}/${ASSET}")"
  [ "$expected" = "$actual" ] ||
    die "checksum mismatch for ${ASSET}: published ${expected}, downloaded ${actual}"

  say "sha256: verified against ${ASSET}.sha256"
  install_binary "${tmp}/${ASSET}"
  prove_install
  say "installed ${PROVEN} at ${TARGET_DIR}/${BINARY}"
}

if [ "$FROM_SOURCE" -eq 1 ]; then
  install_from_source
else
  install_from_release
  report_path
fi

say "next: run \`${BINARY} --version\` and \`${BINARY} config\`"
say "updates later: \`${BINARY} update\`; uninstall: rm ${TARGET_DIR}/${BINARY}"
