# CLI and Local API Automation

Falcon DM's loopback API can be scripted without the browser extension. The repository includes `scripts/falconctl.py`, a Python 3 CLI that uses only the standard library.

## falconctl

Run it directly from the repository:

```bash
python3 scripts/falconctl.py --help
```

The CLI reads `api_token` from:

```text
~/Library/Application Support/com.falcondm.app/settings.json
```

That file is owned by the current user and chmod `0600`. Do not copy the token into shell history, scripts, logs or documentation.

### Commands

```bash
# Add a download
python3 scripts/falconctl.py add 'https://example.com/file.zip'
python3 scripts/falconctl.py add 'https://example.com/video' --name video.mp4 --format 'best[height<=720]'

# List the first page (up to 200 rows), optionally filtered
python3 scripts/falconctl.py list
python3 scripts/falconctl.py list --status paused --category video --search example
python3 scripts/falconctl.py list --json

# Control one row
python3 scripts/falconctl.py pause 42
python3 scripts/falconctl.py resume 42
python3 scripts/falconctl.py remove 42
python3 scripts/falconctl.py remove 42 --delete-file

# Control the queue
python3 scripts/falconctl.py pause-all
python3 scripts/falconctl.py resume-all

# Speed limits: one value = global, id + value = one download
python3 scripts/falconctl.py limit 512
python3 scripts/falconctl.py limit off
python3 scripts/falconctl.py limit 42 256
python3 scripts/falconctl.py limit 42 off

# Queue counters, current speed, free disk and global cap
python3 scripts/falconctl.py stats
```

Filters are case-insensitive. `list` shows the first one-based page (default/maximum page size: 200 rows) and uses the API's privacy-preserving metadata form: URLs, paths, cookies and request headers are not printed. Use the curl form below with `include=url` when a script explicitly needs URLs.

`--wake` can be placed before or after the subcommand:

```bash
python3 scripts/falconctl.py --wake stats
python3 scripts/falconctl.py stats --wake
```

When the connection is refused, `--wake` runs `open falcondm://wake` and retries for up to ten seconds. Normal API requests time out after ten seconds. API failures are printed on stderr and exit nonzero; argparse usage errors exit with code 2.

## curl examples

CLI-style automation sends exactly these two auth headers and **no `Origin` header**:

```text
X-Falcon-Token: <api_token>
X-Falcon-Client: cli
```

Read the token into a shell variable without displaying it:

```bash
TOKEN=$(python3 -c 'import json, pathlib; print(json.loads((pathlib.Path.home()/"Library/Application Support/com.falcondm.app/settings.json").read_text())["api_token"])')
```

Examples:

```bash
# Liveness is unauthenticated
curl -sS http://127.0.0.1:14201/api/health

# Add a download
curl -sS -X POST http://127.0.0.1:14201/api/add \
  -H "X-Falcon-Token: $TOKEN" \
  -H 'X-Falcon-Client: cli' \
  -H 'Content-Type: application/json' \
  --data '{"url":"https://example.com/file.zip","filename":""}'

# First page of paused rows; add include=url only when URLs are required
curl -sS -G http://127.0.0.1:14201/api/downloads/all \
  -H "X-Falcon-Token: $TOKEN" \
  -H 'X-Falcon-Client: cli' \
  --data-urlencode 'status=paused' \
  --data-urlencode 'page=1' \
  --data-urlencode 'page_size=50'

curl -sS -G http://127.0.0.1:14201/api/downloads/all \
  -H "X-Falcon-Token: $TOKEN" \
  -H 'X-Falcon-Client: cli' \
  --data-urlencode 'include=url'

# Queue actions
curl -sS -X POST http://127.0.0.1:14201/api/pause \
  -H "X-Falcon-Token: $TOKEN" -H 'X-Falcon-Client: cli' \
  -H 'Content-Type: application/json' --data '{"id":42}'

curl -sS -X POST http://127.0.0.1:14201/api/resume_all \
  -H "X-Falcon-Token: $TOKEN" -H 'X-Falcon-Client: cli'

# Global cap, per-download cap, then remove a cap
curl -sS -X POST http://127.0.0.1:14201/api/speed_limit \
  -H "X-Falcon-Token: $TOKEN" -H 'X-Falcon-Client: cli' \
  -H 'Content-Type: application/json' --data '{"kbps":512}'

curl -sS -X POST http://127.0.0.1:14201/api/speed_limit \
  -H "X-Falcon-Token: $TOKEN" -H 'X-Falcon-Client: cli' \
  -H 'Content-Type: application/json' --data '{"id":42,"kbps":256}'

curl -sS -X POST http://127.0.0.1:14201/api/speed_limit \
  -H "X-Falcon-Token: $TOKEN" -H 'X-Falcon-Client: cli' \
  -H 'Content-Type: application/json' --data '{"id":42,"kbps":0}'

# Aggregated counters
curl -sS http://127.0.0.1:14201/api/stats \
  -H "X-Falcon-Token: $TOKEN" -H 'X-Falcon-Client: cli'
```

A request with a missing or wrong token returns `401`. A valid token without either an allowlisted extension `Origin` or the exact CLI marker returns `403`. Browser extension auth continues to use `X-Falcon-Token` plus the paired extension `Origin`; the CLI marker never weakens that path.
