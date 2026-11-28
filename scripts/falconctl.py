#!/usr/bin/env python3
"""falconctl — stdlib-only command-line control for Falcon DM's local API."""

import argparse
import errno
import json
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

API_ROOT = "http://127.0.0.1:14201"
TIMEOUT_SECONDS = 10.0
WAKE_TIMEOUT_SECONDS = 10.0

class NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


# Never proxy loopback control traffic, and never follow a redirect that could
# carry the API token away from 127.0.0.1.
_URL_OPENER = urllib.request.build_opener(
    urllib.request.ProxyHandler({}), NoRedirectHandler()
)


class FalconError(Exception):
    """Base class for expected CLI failures."""


class ApiError(FalconError):
    def __init__(self, message, status=None):
        super().__init__(message)
        self.status = status


class ApiConnectionError(FalconError):
    def __init__(self, message, refused=False):
        super().__init__(message)
        self.refused = refused


def settings_file() -> Path:
    return Path.home() / "Library/Application Support/com.falcondm.app/settings.json"


def read_token() -> str:
    path = settings_file()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise FalconError(f"settings file not found: {path}; start Falcon DM once") from None
    except (OSError, json.JSONDecodeError) as exc:
        raise FalconError(f"cannot read API token from {path}: {exc}") from None
    token = data.get("api_token")
    if not isinstance(token, str) or not token.strip():
        raise FalconError(f"settings file has no api_token: {path}")
    return token


def decode_json(status: int, raw: bytes):
    if not raw:
        return {}
    try:
        return json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise ApiError(f"invalid JSON response (HTTP {status})", status) from None


def error_message(status: int, raw: bytes) -> str:
    try:
        data = decode_json(status, raw)
    except ApiError:
        return f"HTTP {status}"
    if isinstance(data, dict) and isinstance(data.get("error"), str):
        return data["error"]
    return f"HTTP {status}"


def is_connection_refused(reason) -> bool:
    seen = set()
    current = reason
    while current is not None and id(current) not in seen:
        seen.add(id(current))
        if isinstance(current, ConnectionRefusedError):
            return True
        if getattr(current, "errno", None) == errno.ECONNREFUSED:
            return True
        current = getattr(current, "__cause__", None) or getattr(current, "__context__", None)
    return False


def api_request_once(path: str, token: str, method="GET", payload=None):
    url = f"{API_ROOT}{path}"
    data = None
    headers = {
        "Accept": "application/json",
        "X-Falcon-Token": token,
        "X-Falcon-Client": "cli",
    }
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with _URL_OPENER.open(request, timeout=TIMEOUT_SECONDS) as response:
            body = response.read()
            result = decode_json(response.status, body)
    except urllib.error.HTTPError as exc:
        raise ApiError(error_message(exc.code, exc.read()), exc.code) from None
    except urllib.error.URLError as exc:
        reason = getattr(exc, "reason", exc)
        raise ApiConnectionError(
            f"cannot connect to Falcon DM at {API_ROOT}: {reason}",
            refused=is_connection_refused(reason),
        ) from None
    except TimeoutError as exc:
        raise ApiConnectionError(f"timed out connecting to Falcon DM: {exc}") from None

    if isinstance(result, dict) and result.get("success") is False:
        raise ApiError(str(result.get("error") or "request failed"), response.status)
    return result


def wake_app() -> None:
    try:
        subprocess.run(
            ["open", "falcondm://wake"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        # The retry loop below still gives an already-starting app a chance.
        pass


def api_request(path: str, token: str, method="GET", payload=None, wake=False):
    try:
        return api_request_once(path, token, method=method, payload=payload)
    except ApiConnectionError as first:
        if not wake or not first.refused:
            raise
        wake_app()
        deadline = time.monotonic() + WAKE_TIMEOUT_SECONDS
        while time.monotonic() < deadline:
            time.sleep(0.25)
            try:
                return api_request_once(path, token, method=method, payload=payload)
            except ApiConnectionError as current:
                if not current.refused:
                    raise
        raise ApiConnectionError("Falcon DM did not start listening within 10 seconds") from None


def format_bytes(value) -> str:
    try:
        amount = float(value or 0)
    except (TypeError, ValueError):
        amount = 0.0
    units = ["B", "KB", "MB", "GB", "TB"]
    index = 0
    while amount >= 1000 and index < len(units) - 1:
        amount /= 1000.0
        index += 1
    if index == 0:
        return f"{int(amount)} B"
    text = f"{amount:.1f}".rstrip("0").rstrip(".")
    return f"{text} {units[index]}"


def format_speed(value) -> str:
    return f"{format_bytes(value)}/s"


def truncate(text: str, width: int) -> str:
    if len(text) <= width:
        return text
    return text[: max(0, width - 1)] + "…"


def print_download_table(rows) -> None:
    if not rows:
        print("no downloads")
        return
    print(f"{'ID':>6}  {'STATUS':<11}  {'PROGRESS':>9}  {'SPEED':>11}  {'SIZE':>17}  NAME")
    for row in rows:
        downloaded = row.get("downloaded_size") or 0
        total = row.get("total_size") or 0
        progress = f"{downloaded * 100 / total:.0f}%" if total else "—"
        size = f"{format_bytes(downloaded)}/{format_bytes(total)}"
        speed = format_speed(row.get("speed") or 0) if row.get("speed") else "—"
        name = truncate(str(row.get("filename") or ""), 48)
        print(
            f"{row.get('id', 0):>6}  {row.get('status', ''):<11}  {progress:>9}  "
            f"{speed:>11}  {size:>17}  {name}"
        )


def positive_id(raw: str) -> int:
    try:
        value = int(raw, 10)
    except ValueError:
        raise argparse.ArgumentTypeError("id must be an integer") from None
    if value <= 0:
        raise argparse.ArgumentTypeError("id must be positive")
    return value


def parse_kbps(raw: str) -> int:
    if raw.lower() == "off":
        return 0
    try:
        value = int(raw, 10)
    except ValueError:
        raise FalconError("speed limit must be a positive KB/s integer or 'off'") from None
    if value < 0 or value > 4_294_967_295:
        raise FalconError("speed limit is outside the unsigned 32-bit API range")
    return value


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="falconctl",
        description="Control a running Falcon DM through its loopback API.",
    )
    parser.add_argument(
        "--wake",
        action="store_true",
        help="try `open falcondm://wake` when the API connection is refused",
    )
    wake_parent = argparse.ArgumentParser(add_help=False)
    wake_parent.add_argument(
        "--wake",
        action="store_true",
        default=argparse.SUPPRESS,
        help="try `open falcondm://wake` when the API connection is refused",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    add = subparsers.add_parser("add", parents=[wake_parent], help="add a URL")
    add.add_argument("url")
    add.add_argument("--name", help="override the output filename")
    add.add_argument("--format", dest="format_selector", help="format selector (e.g. bestvideo[height<=1080]+bestaudio/best)")

    list_cmd = subparsers.add_parser("list", parents=[wake_parent], help="list downloads")
    list_cmd.add_argument("--status", help="status filter (queued, downloading, paused, ...)")
    list_cmd.add_argument("--category", help="category filter (video, music, document, ...)")
    list_cmd.add_argument("--search", help="filename/URL search text")
    list_cmd.add_argument("--json", action="store_true", help="print the rows as JSON")

    for name in ("pause", "resume"):
        cmd = subparsers.add_parser(name, parents=[wake_parent], help=f"{name} one download")
        cmd.add_argument("id", type=positive_id)

    remove = subparsers.add_parser("remove", parents=[wake_parent], help="remove one download")
    remove.add_argument("id", type=positive_id)
    remove.add_argument(
        "--delete-file",
        action="store_true",
        help="also delete the finished file from disk",
    )

    subparsers.add_parser("pause-all", parents=[wake_parent], help="pause all active downloads")
    subparsers.add_parser("resume-all", parents=[wake_parent], help="resume paused/failed downloads")

    limit = subparsers.add_parser(
        "limit",
        parents=[wake_parent],
        usage="falconctl limit [--wake] [id] <kbps|off>",
        help="set the global or per-download speed limit",
    )
    limit.add_argument("target", help="KB/s/off globally, or a download id when value follows")
    limit.add_argument("value", nargs="?", help="KB/s or off")

    subparsers.add_parser("stats", parents=[wake_parent], help="print queue statistics")
    return parser


def command_add(args, token: str) -> None:
    payload = {"url": args.url, "filename": args.name or ""}
    if args.format_selector:
        payload["format"] = args.format_selector
    result = api_request("/api/add", token, method="POST", payload=payload, wake=args.wake)
    print(f"added download {result.get('id')}")


def command_list(args, token: str) -> None:
    query = {}
    if args.status:
        query["status"] = args.status
    if args.category:
        query["category"] = args.category
    if args.search:
        query["search"] = args.search
    suffix = ""
    if query:
        suffix = "?" + urllib.parse.urlencode(query)
    result = api_request(f"/api/downloads/all{suffix}", token, wake=args.wake)
    rows = result.get("downloads", [])
    if args.json:
        print(json.dumps(rows, ensure_ascii=False, indent=2))
    else:
        print_download_table(rows)


def command_pause(args, token: str) -> None:
    api_request(
        "/api/pause",
        token,
        method="POST",
        payload={"id": args.id},
        wake=args.wake,
    )
    print(f"paused download {args.id}")


def command_resume(args, token: str) -> None:
    api_request(
        "/api/resume",
        token,
        method="POST",
        payload={"id": args.id},
        wake=args.wake,
    )
    print(f"resumed download {args.id}")


def command_remove(args, token: str) -> None:
    payload = {"id": args.id}
    if args.delete_file:
        payload["delete_file"] = True
    api_request("/api/remove", token, method="POST", payload=payload, wake=args.wake)
    print(f"removed download {args.id}")


def command_pause_all(args, token: str) -> None:
    result = api_request("/api/pause_all", token, method="POST", wake=args.wake)
    print(f"paused {result.get('paused', 0)} downloads")


def command_resume_all(args, token: str) -> None:
    result = api_request("/api/resume_all", token, method="POST", wake=args.wake)
    print(f"resumed {result.get('resumed', 0)} downloads")


def command_limit(args, token: str) -> None:
    download_id = None
    raw_limit = args.target
    if args.value is not None:
        try:
            download_id = positive_id(args.target)
        except argparse.ArgumentTypeError as exc:
            raise FalconError(f"invalid download id: {exc}") from None
        raw_limit = args.value
    kbps = parse_kbps(raw_limit)
    payload = {"kbps": kbps}
    if download_id is not None:
        payload["id"] = download_id
    api_request("/api/speed_limit", token, method="POST", payload=payload, wake=args.wake)
    limit_text = "off" if kbps == 0 else f"{kbps} KB/s"
    scope = f"download {download_id}" if download_id is not None else "global"
    print(f"{scope} speed limit: {limit_text}")


def command_stats(args, token: str) -> None:
    result = api_request("/api/stats", token, wake=args.wake)
    rows = [
        ("Active", result.get("active", 0)),
        ("Queued", result.get("queued", 0)),
        ("Paused", result.get("paused", 0)),
        ("Completed", result.get("completed", 0)),
        ("Failed", result.get("failed", 0)),
        ("Current speed", format_speed(result.get("current_speed", 0))),
        ("Downloaded", format_bytes(result.get("total_downloaded_bytes", 0))),
    ]
    if result.get("free_disk_bytes") is not None:
        rows.append(("Free disk", format_bytes(result.get("free_disk_bytes"))))
    limit = result.get("speed_limit_kbps", 0)
    rows.append(("Speed limit", "off" if not limit else f"{limit} KB/s"))
    width = max(len(label) for label, _ in rows)
    for label, value in rows:
        print(f"{label:<{width}}  {value}")


def run(args) -> None:
    token = read_token()
    handlers = {
        "add": command_add,
        "list": command_list,
        "pause": command_pause,
        "resume": command_resume,
        "remove": command_remove,
        "pause-all": command_pause_all,
        "resume-all": command_resume_all,
        "limit": command_limit,
        "stats": command_stats,
    }
    handlers[args.command](args, token)


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    try:
        run(args)
    except FalconError as exc:
        print(f"falconctl: error: {exc}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main())
