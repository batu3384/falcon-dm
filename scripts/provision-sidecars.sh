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

verify_sha256() {
  local file="$1"
  local expected="$2"
  local actual

  [[ -f "$file" ]] || {
    echo "checksum target does not exist: $file" >&2
    return 1
  }
  [[ "$expected" =~ ^[[:xdigit:]]{64}$ ]] || {
    echo "invalid SHA-256 value for $file" >&2
    return 1
  }
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "SHA-256 mismatch for $file" >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    return 1
  fi
}

real_path() {
  if command -v realpath >/dev/null 2>&1; then
    realpath "$1"
  else
    python3 - "$1" <<'PY'
import os
import sys

print(os.path.realpath(sys.argv[1]))
PY
  fi
}

copy_verified() {
  local source="$1"
  local destination="$2"
  local expected="${3:-}"

  cp "$source" "$destination"
  chmod +x "$destination"
  if [[ -n "$expected" ]]; then
    if ! verify_sha256 "$destination" "$expected"; then
      rm -f "$destination"
      return 1
    fi
  fi
}

main() {
  local script_root
  script_root="$(cd "$(dirname "$0")/.." && pwd)"
  local binary_dir="$script_root/src-tauri/binaries"
  mkdir -p "$binary_dir"

  local arch="${ARCH:-$(uname -m)}"
  # Normalize Apple Silicon arch name
  if [[ "$arch" == "arm64" ]]; then
    arch="aarch64"
  fi
  local target_triple="${arch}-apple-darwin"
  local aria2_sha256="${ARIA2_SHA256:-}"
  local ffmpeg_url="${FFMPEG_URL:-}"
  local ffmpeg_sha256="${FFMPEG_SHA256:-}"

  echo "Provisioning sidecars for $target_triple in $binary_dir"

  # ----- aria2c -----
  local aria2_bin="$binary_dir/aria2c-$target_triple"
  if [[ -x "$aria2_bin" ]]; then
    if [[ -n "$aria2_sha256" ]]; then
      verify_sha256 "$aria2_bin" "$aria2_sha256"
    fi
    echo "  aria2c already present, skipping"
  else
    echo "  aria2c missing — provisioning from Homebrew"
    local brew_bin
    if [[ "$arch" == "aarch64" ]]; then
      brew_bin="/opt/homebrew/bin/aria2c"
    else
      brew_bin="/usr/local/bin/aria2c"
    fi
    if [[ -x "$brew_bin" ]]; then
      # Homebrew verifies its bottle; optionally pin the copied binary too.
      local real
      real="$(real_path "$brew_bin")"
      copy_verified "$real" "$aria2_bin" "$aria2_sha256"
      echo "  aria2c copied from $real"
    else
      echo "  ERROR: aria2c not found at $brew_bin" >&2
      echo "  Install it: brew install aria2" >&2
      echo "  (for x86_64 on Apple Silicon, install Rosetta + x86 Homebrew first)" >&2
      exit 1
    fi
  fi
 
  # ----- ffmpeg -----
  local ffmpeg_bin="$binary_dir/ffmpeg-$target_triple"
  if [[ -x "$ffmpeg_bin" ]]; then
    if [[ "$arch" == "x86_64" && -z "$ffmpeg_sha256" ]]; then
      echo "  ERROR: FFMPEG_SHA256 is required to verify an Intel download" >&2
      exit 1
    fi
    if [[ -n "$ffmpeg_sha256" ]]; then
      verify_sha256 "$ffmpeg_bin" "$ffmpeg_sha256"
    fi
    echo "  ffmpeg already present, skipping"
  else
    echo "  ffmpeg missing — provisioning"
    if [[ "$arch" == "aarch64" ]]; then
      local brew_bin="/opt/homebrew/bin/ffmpeg"
      if [[ -x "$brew_bin" ]]; then
        local real
        real="$(real_path "$brew_bin")"
        copy_verified "$real" "$ffmpeg_bin" "$ffmpeg_sha256"
        echo "  ffmpeg copied from $real"
      else
        echo "  ERROR: ffmpeg not at $brew_bin" >&2
        exit 1
      fi
    else
      [[ -n "$ffmpeg_url" ]] || {
        echo "  ERROR: FFMPEG_URL is required for remote ffmpeg downloads" >&2
        exit 1
      }
      [[ -n "$ffmpeg_sha256" ]] || {
        echo "  ERROR: FFMPEG_SHA256 is required for remote ffmpeg downloads" >&2
        exit 1
      }
      local tmp
      tmp="$(mktemp -d)"
      curl --fail --location --retry 3 --retry-delay 1 --silent --show-error \
        --output "$tmp/ffmpeg.zip" "$ffmpeg_url"
      (cd "$tmp" && unzip -o ffmpeg.zip >/dev/null 2>&1)
      [[ -x "$tmp/ffmpeg" ]] || {
        echo "  ERROR: failed to extract ffmpeg" >&2
        exit 1
      }
      verify_sha256 "$tmp/ffmpeg" "$ffmpeg_sha256"
      copy_verified "$tmp/ffmpeg" "$ffmpeg_bin"
      rm -rf "$tmp"
      echo "  ffmpeg downloaded from $ffmpeg_url"
    fi
  fi

  echo "Done. Sidecars for $target_triple:"
  ls -la "$binary_dir"/*-"$target_triple"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
