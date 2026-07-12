#!/bin/sh
# Jeliya installer (POSIX sh). Mirrors the iroh / sendme install recipe:
# detect platform, download the matching release archive, drop `jeliyad` on
# PATH. This script does NOT run the binary.
#
# Requires a published GitHub Release with jeliyad assets attached.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/kortiene/jeliya/main/packaging/install.sh | sh
#
# Env overrides:
#   JELIYA_VERSION=v0.1.0   pin a specific release tag (default: latest)
#   INSTALL_DIR=/some/bin    install location (default: /usr/local/bin, else ~/.local/bin)
set -eu

# --- config -----------------------------------------------------------------
REPO="kortiene/jeliya"
BIN="jeliyad"

VERSION="${JELIYA_VERSION:-}"
INSTALL_DIR="${INSTALL_DIR:-}"

err() { printf 'error: %s\n' "$1" >&2; exit 1; }
info() { printf '%s\n' "$1" >&2; }

# --- pick a downloader ------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  DL="curl"
elif command -v wget >/dev/null 2>&1; then
  DL="wget"
else
  err "need either curl or wget on PATH"
fi

download() { # download <url> <dest-file>
  if [ "$DL" = "curl" ]; then
    curl -fsSL "$1" -o "$2"
  else
    wget -q "$1" -O "$2"
  fi
}

sha256_file() { # sha256_file <path> -> lowercase hex digest
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$1" | awk '{print $NF}'
  else
    err "need sha256sum, shasum, or openssl to verify the downloaded archive"
  fi
}

fetch_stdout() { # fetch_stdout <url>  -> prints body to stdout
  if [ "$DL" = "curl" ]; then
    curl -fsSL "$1"
  else
    wget -qO- "$1"
  fi
}

# --- detect platform --------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin) os_part="apple-darwin" ;;
  Linux) os_part="unknown-linux-musl" ;;
  *) err "unsupported OS: $os (this installer supports macOS and Linux)" ;;
esac

case "$arch" in
  x86_64 | amd64) arch_part="x86_64" ;;
  arm64 | aarch64) arch_part="aarch64" ;;
  *) err "unsupported architecture: $arch (supported: x86_64, aarch64)" ;;
esac

TARGET="${arch_part}-${os_part}"

# --- resolve version --------------------------------------------------------
# Release assets are versioned (jeliyad-<tag>-<target>.tar.gz), so the static
# /releases/latest/download/ alias cannot name them. Resolve the latest tag via
# the GitHub API (as sendme does) unless JELIYA_VERSION pins one.
if [ -z "$VERSION" ]; then
  info "resolving latest release of ${REPO} ..."
  VERSION="$(fetch_stdout "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -n1 | cut -d'"' -f4 || true)"
  [ -n "$VERSION" ] || err "could not resolve latest version; set JELIYA_VERSION=vX.Y.Z to pin one"
fi

ASSET="${BIN}-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"
CHECKSUM="${ASSET}.sha256"

# --- download + extract -----------------------------------------------------
tmp="$(mktemp -d 2>/dev/null || mktemp -d -t jeliya)"
trap 'rm -rf "$tmp"' EXIT INT TERM

info "downloading ${ASSET} ..."
download "$URL" "$tmp/$ASSET" || err "download failed: $URL"
[ -s "$tmp/$ASSET" ] || err "downloaded archive is empty or missing: $URL"

info "downloading and verifying ${CHECKSUM} ..."
download "${URL}.sha256" "$tmp/$CHECKSUM" \
  || err "checksum download failed: ${URL}.sha256"
[ -s "$tmp/$CHECKSUM" ] || err "downloaded checksum is empty or missing: ${URL}.sha256"

# A sidecar is trusted only when it has one non-empty two-field line, a
# 64-hex digest, and names this exact archive. This prevents a valid digest
# for a different release/target from being applied accidentally.
awk 'NF { if (seen || NF != 2) exit 1; seen = 1 } END { if (!seen) exit 1 }' \
  "$tmp/$CHECKSUM" || err "invalid checksum sidecar format: $CHECKSUM"
expected="$(awk 'NF { print $1 }' "$tmp/$CHECKSUM")"
listed="$(awk 'NF { print $2 }' "$tmp/$CHECKSUM")"
[ "$listed" = "$ASSET" ] || err "checksum sidecar names '$listed', expected '$ASSET'"
[ "${#expected}" -eq 64 ] || err "checksum is not 64 hexadecimal characters"
case "$expected" in
  *[!0-9A-Fa-f]*) err "checksum is not 64 hexadecimal characters" ;;
esac
actual="$(sha256_file "$tmp/$ASSET" | tr 'A-F' 'a-f')"
expected="$(printf '%s' "$expected" | tr 'A-F' 'a-f')"
[ "$actual" = "$expected" ] || err "checksum mismatch for $ASSET"
info "checksum verified"

members="$(tar -tzf "$tmp/$ASSET")" || err "could not inspect $ASSET"
[ "$members" = "$BIN" ] || err "archive must contain exactly '$BIN'"

info "extracting ..."
tar -xzf "$tmp/$ASSET" -C "$tmp" || err "failed to extract $ASSET"
[ -f "$tmp/$BIN" ] && [ ! -L "$tmp/$BIN" ] \
  || err "archive did not contain a regular '$BIN' file"
chmod +x "$tmp/$BIN"

# --- choose install dir -----------------------------------------------------
if [ -n "$INSTALL_DIR" ]; then
  dest="$INSTALL_DIR"
elif [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then
  dest="/usr/local/bin"
else
  dest="$HOME/.local/bin"
fi

mkdir -p "$dest" || err "cannot create install dir: $dest"
mv "$tmp/$BIN" "$dest/$BIN" || err "cannot write to $dest (set INSTALL_DIR=... or re-run with sudo)"
chmod +x "$dest/$BIN"

info ""
info "installed ${BIN} ${VERSION} -> ${dest}/${BIN}"

# PATH hint if the chosen dir is not already on PATH.
case ":$PATH:" in
  *":$dest:"*) : ;;
  *) info "note: ${dest} is not on your PATH -- add it, e.g.:  export PATH=\"${dest}:\$PATH\"" ;;
esac

info ""
info "next: run \`${BIN}\` -- it opens the Jeliya UI in your browser."
