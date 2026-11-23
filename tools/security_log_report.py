#!/usr/bin/env python3
"""Read-only bounded summary of TOR public-edge Nginx sanitized security log."""
from __future__ import annotations

import argparse
from collections import Counter, deque
from datetime import datetime, timedelta, timezone
import ipaddress
import json
import math
from pathlib import Path
import sys

FIELDS = {"ts", "ip", "method", "status", "route", "limiter", "rt"}
ROUTES = {"other", "auth", "trap", "admin", "account", "reading", "discovery", "static"}
METHODS = {"GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"}
MAX_LINES = 100_000


def parse_line(line: str) -> dict | None:
    parts = line.rstrip("\n").split(" ")
    if len(parts) != len(FIELDS):
        return None
    fields = {}
    for part in parts:
        key, separator, value = part.partition("=")
        if separator != "=" or key in fields:
            return None
        fields[key] = value
    if set(fields) != FIELDS:
        return None
    try:
        stamp = datetime.fromisoformat(fields["ts"])
        ip = ipaddress.ip_address(fields["ip"])
        status = int(fields["status"])
        duration = float(fields["rt"])
    except (ValueError, OverflowError):
        return None
    if (stamp.tzinfo is None or fields["method"] not in METHODS
            or fields["route"] not in ROUTES
            or not 100 <= status <= 599
            or not math.isfinite(duration) or not 0 <= duration <= 3600
            or fields["limiter"] not in {
                "", "-", "PASSED", "DELAYED", "REJECTED",
                "DELAYED_DRY_RUN", "REJECTED_DRY_RUN",
            }):
        return None
    return {"ts": stamp, "ip": ip, "method": fields["method"],
            "status": status, "route": fields["route"],
            "limiter": fields["limiter"]}


def source_label(ip: ipaddress.IPv4Address | ipaddress.IPv6Address, *, full: bool) -> str:
    if full:
        return str(ip)
    prefix = 24 if ip.version == 4 else 64
    return str(ipaddress.ip_network(f"{ip}/{prefix}", strict=False))


def summarize(lines, *, now: datetime, minutes: int = 15, show_full_ip: bool = False) -> dict:
    if now.tzinfo is None or not 1 <= minutes <= 24 * 60:
        raise ValueError("Need an aware time and a window of 1..1440 minutes.")
    lower = now - timedelta(minutes=minutes)
    statuses, routes, limiter = Counter(), Counter(), Counter()
    suspects = Counter()
    events = excluded = 0
    for raw in lines:
        row = parse_line(raw)
        if row is None or not lower <= row["ts"] <= now:
            excluded += 1
            continue
        events += 1
        statuses[str(row["status"])] += 1
        routes[row["route"]] += 1
        limiter[row["limiter"] or "-"] += 1
        if (row["status"] in {403, 404, 429, 444}
                or row["route"] == "trap"
                or "REJECTED" in row["limiter"]):
            suspects[source_label(row["ip"], full=show_full_ip)] += 1
    return {
        "schema": "tor.nginx_security_window.v1",
        "minutes": minutes,
        "requests": events,
        "statuses": dict(sorted(statuses.items())),
        "route_groups": dict(sorted(routes.items())),
        "limiter_results": dict(sorted(limiter.items())),
        "candidate_sources": [
            {"source": key, "signals": hits}
            for key, hits in sorted(suspects.items(), key=lambda item: (-item[1], item[0]))[:10]
        ],
        "outside_window_or_invalid_lines": excluded,
        "note": "Signals are requests, not unique bots or proof of an attack.",
        "privacy": "Exact IPs shown only on explicit --show-full-ip.",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--file", type=Path,
                        default=Path("/var/log/nginx/tor-security.log"))
    parser.add_argument("--minutes", type=int, default=15)
    parser.add_argument("--show-full-ip", action="store_true",
                        help="Sensitive: reveal exact source IPs to the local operator.")
    args = parser.parse_args()
    if not 1 <= args.minutes <= 1440:
        print("TOR_SECURITY_REPORT_INVALID_WINDOW", file=sys.stderr)
        return 2
    try:
        with args.file.open(encoding="utf-8", errors="replace") as stream:
            snapshot = list(deque(stream, maxlen=MAX_LINES))
    except OSError:
        print("TOR_SECURITY_REPORT_CANNOT_READ_LOG", file=sys.stderr)
        return 2
    result = summarize(snapshot, now=datetime.now(timezone.utc),
                       minutes=args.minutes, show_full_ip=args.show_full_ip)
    result["max_examined_lines"] = MAX_LINES
    json.dump(result, sys.stdout, ensure_ascii=False, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
