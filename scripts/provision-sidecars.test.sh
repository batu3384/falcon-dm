#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/provision-sidecars.sh"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

printf 'falcon-sidecar-fixture' > "$tmp/fixture"
expected="$(shasum -a 256 "$tmp/fixture" | awk '{print $1}')"
verify_sha256 "$tmp/fixture" "$expected"

if verify_sha256 "$tmp/fixture" "0000000000000000000000000000000000000000000000000000000000000000"; then
  echo "verify_sha256 accepted an incorrect checksum" >&2
  exit 1
fi

echo "provision-sidecars checksum tests passed"
