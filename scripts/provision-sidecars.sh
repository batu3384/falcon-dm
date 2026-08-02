#!/usr/bin/env bash
# ponytail: provision aria2c + ffmpeg sidecar binaries for both macOS architectures.
#
# The repo ships arm64 (Apple Silicon) binaries. Intel Macs need x86_64 builds.
# aria2 has no official macOS binary release, so we build/copy from Homebrew.
# ffmpeg x86_64 comes from evermeet.cx (static).
#
# Usage:
#   ./scripts/provision-sidecars.sh            # provision whatever's missing
#   ARCH=x86_64 ./scripts/provision-sidecars.sh # force a specific arch
#
# CI (Faz 3) runs this before `tauri build` on a macOS-13 (x86_64) runner to
# produce Intel artifacts, and on macOS-14 (arm64) for Apple Silicon.
#
# This script is idempotent — it skips binaries that already exist.
set -euo pipefail

BINARY_DIR="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/binaries"
mkdir -p "$BINARY_DIR"

ARCH="${ARCH:-$(uname -m)}"
# Normalize Apple Silicon arch name
if [[ "$ARCH" == "arm64" ]]; then
  ARCH="aarch64"
fi
TARGET_TRIPLE="${ARCH}-apple-darwin"

echo "Provisioning sidecars for $TARGET_TRIPLE in $BINARY_DIR"

# ----- aria2c -----
ARIA2_BIN="$BINARY_DIR/aria2c-$TARGET_TRIPLE"
if [[ -x "$ARIA2_BIN" ]]; then
  echo "  aria2c already present, skipping"
else
  echo "  aria2c missing — provisioning from Homebrew"
  if [[ "$ARCH" == "aarch64" ]]; then
    BREW_BIN="/opt/homebrew/bin/aria2c"
  else
    BREW_BIN="/usr/local/bin/aria2c"
  fi
  if [[ -x "$BREW_BIN" ]]; then
    # Copy the real file (resolve symlinks) and strip signature for cross-copy
    REAL="$(readlink -f "$BREW_BIN")"
    cp "$REAL" "$ARIA2_BIN"
    chmod +x "$ARIA2_BIN"
    echo "  aria2c copied from $REAL"
  else
    echo "  ERROR: aria2c not found at $BREW_BIN" >&2
    echo "  Install it:  arch -${ARCH/arm64/arm64} brew install aria2" >&2
    echo "  (for x86_64 on Apple Silicon, install Rosetta + x86 Homebrew first)" >&2
    exit 1
  fi
fi

# ----- ffmpeg -----
FFMPEG_BIN="$BINARY_DIR/ffmpeg-$TARGET_TRIPLE"
if [[ -x "$FFMPEG_BIN" ]]; then
  echo "  ffmpeg already present, skipping"
else
  echo "  ffmpeg missing — provisioning"
  if [[ "$ARCH" == "aarch64" ]]; then
    BREW_BIN="/opt/homebrew/bin/ffmpeg"
    if [[ -x "$BREW_BIN" ]]; then
      REAL="$(readlink -f "$BREW_BIN")"
      cp "$REAL" "$FFMPEG_BIN"
      chmod +x "$FFMPEG_BIN"
      echo "  ffmpeg copied from $REAL"
    else
      echo "  ERROR: ffmpeg not at $BREW_BIN" >&2; exit 1
    fi
  else
    # x86_64: evermeet.cx static build
    TMP="$(mktemp -d)"
    curl -sL -o "$TMP/ffmpeg.zip" "https://evermeet.cx/ffmpeg/getrelease/zip"
    (cd "$TMP" && unzip -o ffmpeg.zip >/dev/null 2>&1)
    if [[ -x "$TMP/ffmpeg" ]]; then
      cp "$TMP/ffmpeg" "$FFMPEG_BIN"
      chmod +x "$FFMPEG_BIN"
      echo "  ffmpeg downloaded from evermeet.cx"
    else
      echo "  ERROR: failed to download ffmpeg" >&2; exit 1
    fi
    rm -rf "$TMP"
  fi
fi

echo "Done. Sidecars for $TARGET_TRIPLE:"
ls -la "$BINARY_DIR"/*-"$TARGET_TRIPLE"
