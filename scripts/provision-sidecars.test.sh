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

if (cd "$ROOT" && RELEASE_MODE=1 ARCH=aarch64 ARIA2_SHA256="$expected" main); then
  echo "release mode accepted missing ffmpeg variables" >&2
  exit 1
fi

python3 - "$ROOT/.github/workflows/release.yml" <<'PY'
from pathlib import Path
import sys

workflow = Path(sys.argv[1]).read_text()
required = (
    "Import Apple certificate",
    "APPLE_CERTIFICATE_PASSWORD",
    "APPLE_SIGNING_CONFIGURED",
    'RELEASE_MODE: "1"',
    'test -x "src-tauri/binaries/aria2c-${{ matrix.target }}"',
)
missing = [needle for needle in required if needle not in workflow]
if missing:
    raise SystemExit(f"release workflow contract missing: {missing}")
if "if: ${{ secrets.APPLE_CERTIFICATE" in workflow:
    raise SystemExit("release workflow must not use secrets directly in if conditions")
PY

echo "provision-sidecars checksum tests passed"
