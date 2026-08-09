#!/usr/bin/env bash
set -euo pipefail

: "${NATIVE_HOST_BIN:?NATIVE_HOST_BIN is required}"
: "${CHROME_EXTENSION_ID:?CHROME_EXTENSION_ID is required}"
: "${EDGE_EXTENSION_ID:?EDGE_EXTENSION_ID is required}"

if [[ ! "$CHROME_EXTENSION_ID" =~ ^[a-p]{32}$ ]]; then
  echo "invalid Chrome extension id" >&2
  exit 1
fi
if [[ ! "$EDGE_EXTENSION_ID" =~ ^[a-p]{32}$ ]]; then
  echo "invalid Edge extension id" >&2
  exit 1
fi
if [[ ! -x "$NATIVE_HOST_BIN" ]]; then
  echo "native host is not executable: $NATIVE_HOST_BIN" >&2
  exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required to install native host manifests" >&2
  exit 1
fi

install_manifest() {
  local target_dir="$1"
  local browser_origin="$2"
  local manifest_path="$target_dir/com.falcondm.native.json"
  local temp_path

  mkdir -p "$target_dir"
  chmod 700 "$target_dir"
  temp_path="$(mktemp "$target_dir/.com.falcondm.native.XXXXXX")"
  python3 - "$temp_path" "$NATIVE_HOST_BIN" "$browser_origin" <<'PY'
import json
import os
import sys

path, executable, origin = sys.argv[1:]
with open(path, "w", encoding="utf-8") as handle:
    json.dump(
        {
            "name": "com.falcondm.native",
            "description": "Falcon DM native pairing host",
            "path": os.path.abspath(executable),
            "type": "stdio",
            "allowed_origins": [origin],
        },
        handle,
        indent=2,
    )
    handle.write("\n")
os.chmod(path, 0o600)
PY
  mv "$temp_path" "$manifest_path"
  chmod 600 "$manifest_path"
}

install_manifest \
  "$HOME/Library/Application Support/Google/Chrome/NativeMessagingHosts" \
  "chrome-extension://${CHROME_EXTENSION_ID}/"
install_manifest \
  "$HOME/Library/Application Support/Microsoft Edge/NativeMessagingHosts" \
  "edge-extension://${EDGE_EXTENSION_ID}/"

echo "Installed com.falcondm.native for Chrome and Edge"
