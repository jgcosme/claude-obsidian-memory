#!/bin/bash
# obsidian-memory binary installer.
#
# Idempotently downloads the matching prebuilt obsidian-memory binary from
# this plugin's GitHub Releases into <plugin-root>/bin/obsidian-memory.
# Detects platform via `uname -s -m`; falls through with a clear error if
# the platform isn't covered by the release matrix.
#
# Invoked lazily by bin/run on first hook fire. Also safe to run by hand:
#   bash <plugin-root>/bin/install.sh
#
# Surface that survives the bash → Rust port: this file plus bin/run. Both
# stay shell because they bootstrap the binary itself; everything downstream
# is Rust.

set -eu

# --- locate plugin root ---
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PLUGIN_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET="$PLUGIN_ROOT/bin/obsidian-memory"

# --- read version from plugin.json (no jq dep — single grep + sed) ---
PLUGIN_MANIFEST="$PLUGIN_ROOT/.claude-plugin/plugin.json"
if [ ! -f "$PLUGIN_MANIFEST" ]; then
  echo "[install] plugin.json not found at $PLUGIN_MANIFEST" >&2
  exit 1
fi
VERSION=$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' "$PLUGIN_MANIFEST" \
          | head -1 \
          | sed -E 's/.*"version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
if [ -z "${VERSION:-}" ]; then
  echo "[install] could not parse version from $PLUGIN_MANIFEST" >&2
  exit 1
fi

# --- already installed? ---
if [ -x "$TARGET" ]; then
  installed=$("$TARGET" --version 2>/dev/null | awk '{print $NF}' || echo "")
  if [ "$installed" = "$VERSION" ]; then
    exit 0
  fi
fi

# --- detect platform ---
OS=$(uname -s)
ARCH=$(uname -m)
case "$OS-$ARCH" in
  Darwin-arm64)        PLATFORM="aarch64-apple-darwin" ;;
  Darwin-x86_64)       PLATFORM="x86_64-apple-darwin" ;;
  Linux-x86_64)        PLATFORM="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64)       PLATFORM="aarch64-unknown-linux-gnu" ;;
  Linux-arm64)         PLATFORM="aarch64-unknown-linux-gnu" ;;
  *)
    echo "[install] unsupported platform: $OS-$ARCH" >&2
    echo "[install] supported: Darwin-arm64, Darwin-x86_64, Linux-x86_64, Linux-aarch64" >&2
    echo "[install] fallback: build from source via 'cargo build --release' in $PLUGIN_ROOT" >&2
    exit 1
    ;;
esac

# --- pick downloader ---
if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
else
  echo "[install] need either curl or wget on PATH" >&2
  exit 1
fi

# --- download tarball + sha256 ---
TARBALL="obsidian-memory-v${VERSION}-${PLATFORM}.tar.gz"
BASE_URL="https://github.com/jgcosme/claude-obsidian-memory/releases/download/v${VERSION}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "[install] downloading ${TARBALL}…" >&2
if ! fetch "$BASE_URL/$TARBALL" "$TMP/$TARBALL"; then
  echo "[install] failed to download $BASE_URL/$TARBALL" >&2
  echo "[install] check that the release exists, then re-run." >&2
  exit 1
fi

# Optional sha256 verification — skip silently if the .sha256 file isn't
# part of the release (e.g., older releases or hand-rolled tags).
if fetch "$BASE_URL/${TARBALL}.sha256" "$TMP/${TARBALL}.sha256" 2>/dev/null; then
  if command -v shasum >/dev/null 2>&1; then
    (cd "$TMP" && shasum -a 256 -c "${TARBALL}.sha256" >/dev/null) || {
      echo "[install] sha256 mismatch for $TARBALL — refusing to install" >&2
      exit 1
    }
  elif command -v sha256sum >/dev/null 2>&1; then
    (cd "$TMP" && sha256sum -c "${TARBALL}.sha256" >/dev/null) || {
      echo "[install] sha256 mismatch for $TARBALL — refusing to install" >&2
      exit 1
    }
  fi
fi

# --- extract + place ---
tar -xzf "$TMP/$TARBALL" -C "$TMP"
EXTRACTED=$(find "$TMP" -name obsidian-memory -type f -perm -u+x | head -1)
if [ -z "$EXTRACTED" ]; then
  echo "[install] extracted tarball did not contain obsidian-memory binary" >&2
  exit 1
fi

mkdir -p "$PLUGIN_ROOT/bin"
mv "$EXTRACTED" "$TARGET"
chmod +x "$TARGET"

echo "[install] installed obsidian-memory v${VERSION} → $TARGET" >&2
