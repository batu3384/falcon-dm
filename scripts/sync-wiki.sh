#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WIKI_SRC="$ROOT/wiki"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if [[ ! -d "$WIKI_SRC" ]]; then
  echo "wiki source missing: $WIKI_SRC" >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "gh CLI required (brew install gh)" >&2
  exit 1
fi

REPO="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
TOKEN="$(gh auth token)"
URL="https://x-access-token:${TOKEN}@github.com/${REPO}.wiki.git"

if ! git clone "$URL" "$TMP/repo" 2>"$TMP/clone.err"; then
  cat >&2 <<EOF
Could not clone ${REPO}.wiki.git

$(cat "$TMP/clone.err" 2>/dev/null || true)

GitHub creates the wiki git backend only after the first wiki page exists.

One-time bootstrap:
  1. Open https://github.com/${REPO}/wiki
  2. Click "Create the first page"
  3. Title: Home — body can be "# Falcon DM" — Save
  4. Re-run: ./scripts/sync-wiki.sh

EOF
  exit 1
fi

# ponytail: never rsync --delete over .git; GitHub wiki uses branch master.
rsync -a --delete --exclude='.git' --exclude='README.md' "$WIKI_SRC/" "$TMP/repo/"
cd "$TMP/repo"
BRANCH="$(git symbolic-ref --short HEAD 2>/dev/null || echo master)"

git add -A
if git diff --cached --quiet; then
  echo "Wiki already up to date."
  exit 0
fi

git -c user.name="Falcon DM" -c user.email="wiki@falcon-dm.local" \
  commit -m "docs: sync wiki from repository wiki/ directory"
git push origin "$BRANCH"

echo "Wiki synced: https://github.com/${REPO}/wiki"
