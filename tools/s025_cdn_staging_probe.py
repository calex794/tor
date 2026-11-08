#!/usr/bin/env python3
"""S025: read-only staging CDN edge probe; zero network by default.

Validates one immutable cover URL before/after revocation, browser TTL and
content SHA-256. NEVER issues purge, POST, PUT, DELETE or S3 operations.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

MAX_BYTES = 8 * 1024 * 1024
IMMUTABLE_PATH = re.compile(
    r"/public/covers/[1-9][0-9]{0,19}/[1-9][0-9]{0,19}/"
    r"(?:card|main)-([0-9a-f]{64})\.webp\Z"
)
HOST = re.compile(r"[a-z0-9-]+(?:\.[a-z0-9-]+)+\Z")
STAGING_LABELS = {"staging", "stage", "test", "testing"}
FORBIDDEN_LABELS = {"prod", "production", "live"}
NEGATIVE_CODES = {403, 404, 410}


class ProbeFailure(ValueError):
    """Unsafe target or failed check; never include the input URL."""


@dataclass(frozen=True)
class Result:
    status: int
    content_type: str
    cache_control: str
    sha256: str | None


class NoRedirect(HTTPRedirectHandler):
    """Prevent redirects to another host or an alternate cache path."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def validate_target(url: str, staging_host: str) -> str:
    """Require an exact immutable WebP path and explicit staging DNS host."""
    if not url or url != url.strip() or not staging_host:
        raise ProbeFailure("missing exact staging URL or host")
    try:
        target = urlsplit(url)
        port = target.port
    except ValueError as exc:
        raise ProbeFailure("invalid URL authority") from exc
    hostname = (target.hostname or "").lower()
    if (target.scheme != "https" or port not in (None, 443)
            or target.username is not None or target.password is not None
            or target.query or target.fragment or not HOST.fullmatch(hostname)
            or hostname != staging_host.lower()):
        raise ProbeFailure("target must use HTTPS on the exact staging hostname")
    labels = set(re.split(r"[.-]", hostname))
    if labels.isdisjoint(STAGING_LABELS) or not labels.isdisjoint(FORBIDDEN_LABELS):
        raise ProbeFailure("production or non-staging hostname refused")
    match = IMMUTABLE_PATH.fullmatch(target.path)
    if not match:
        raise ProbeFailure("URL must address exactly one immutable S025 WebP")
    return match.group(1)


def browser_max_age(cache_control: str) -> int:
    """no-store/no-cache are stronger than a 60-second freshness limit."""
    directives = [item.strip().lower() for item in cache_control.split(",")]
    if not cache_control.strip() or "immutable" in directives:
        raise ProbeFailure("missing or immutable browser cache policy")
    if "no-store" in directives or "no-cache" in directives:
        return 0
    values = [item[8:].strip() for item in directives if item.startswith("max-age=")]
    if len(values) != 1 or not values[0].isdigit():
        raise ProbeFailure("browser max-age is missing or ambiguous")
    age = int(values[0])
    if age > 60:
        raise ProbeFailure("browser freshness exceeds 60 seconds")
    return age


def read_only_get(url: str) -> Result:
    """Bounded GET without auth/cookies/redirects; never leak response bodies."""
    request = Request(url, headers={
        "Accept": "image/webp",
        "User-Agent": "TOR-S025-Staging-Read-Only/1.0",
    }, method="GET")
    opener = build_opener(NoRedirect())
    try:
        with opener.open(request, timeout=8) as response:
            status = response.status
            checksum = None
            if status == 200:
                body = response.read(MAX_BYTES + 1)
                if len(body) > MAX_BYTES:
                    raise ProbeFailure("WebP response is larger than 8 MiB")
                checksum = hashlib.sha256(body).hexdigest()
            return Result(status, response.headers.get("Content-Type", ""),
                          response.headers.get("Cache-Control", ""), checksum)
    except HTTPError as error:
        return Result(error.code, error.headers.get("Content-Type", ""),
                      error.headers.get("Cache-Control", ""), None)
    except (URLError, TimeoutError, OSError) as exc:
        raise ProbeFailure("staging GET failed or timed out") from exc


def evaluate(phase: str, expected_sha256: str, get_result, *,
             probes: int = 3, interval: float = 2.0,
             pause=time.sleep) -> dict:
    """Pure acceptance logic with injectible offline responses for CI tests."""
    if phase == "before":
        result = get_result()
        if result.status != 200:
            raise ProbeFailure("before phase did not return HTTP 200")
        if result.content_type.split(";")[0].strip().lower() != "image/webp":
            raise ProbeFailure("before phase did not return image/webp")
        if result.sha256 != expected_sha256:
            raise ProbeFailure("WebP bytes do not match immutable path SHA-256")
        age = browser_max_age(result.cache_control)
        return {"result": "PASS", "phase": "before", "http_status": 200,
                "sha256": "verified", "browser_max_age_seconds": age}
    if phase != "after" or not 3 <= probes <= 5 or not 0 <= interval <= 5:
        raise ProbeFailure("invalid bounded after-phase settings")
    statuses = []
    for index in range(probes):
        result = get_result()
        statuses.append(result.status)
        if result.status not in NEGATIVE_CODES:
            raise ProbeFailure("old CDN URL serves bytes or an unexpected status")
        if index + 1 < probes:
            pause(interval)
    return {"result": "PASS", "phase": "after", "statuses": statuses,
            "probes": probes, "old_bytes_served": False}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--phase", choices=("before", "after"), required=True)
    parser.add_argument("--url", required=True)
    parser.add_argument("--staging-host", required=True)
    parser.add_argument("--execute-readonly", action="store_true",
                        help="Permit read-only HTTPS GET; never sends purge or writes")
    parser.add_argument("--probes", type=int, default=3)
    parser.add_argument("--interval", type=float, default=2.0)
    args = parser.parse_args(argv)
    try:
        digest = validate_target(args.url, args.staging_host)
        if args.phase == "after" and not (3 <= args.probes <= 5
                                           and 0 <= args.interval <= 5):
            raise ProbeFailure("invalid after-phase probe settings")
        if not args.execute_readonly:
            print(json.dumps({"result": "PREFLIGHT_ONLY", "phase": args.phase,
                              "network_requests": 0, "purge_requests": 0}))
            return 0
        outcome = evaluate(args.phase, digest, lambda: read_only_get(args.url),
                           probes=args.probes, interval=args.interval)
        outcome["checked_at_utc"] = datetime.now(timezone.utc).isoformat()
        outcome["cdn_host_verified"] = True
        outcome["purge_requests"] = 0
        print(json.dumps(outcome, ensure_ascii=False, sort_keys=True))
        return 0
    except ProbeFailure as error:
        print(json.dumps({"result": "FAIL", "phase": args.phase,
                          "reason": str(error), "purge_requests": 0}), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
