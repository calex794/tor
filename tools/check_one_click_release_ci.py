#!/usr/bin/env python3
"""Wait for push checks on the exact TOR main SHA before any VPS SSH.

GitHub path filters mean a documentation-only commit may run Repository rules
without PostgreSQL/visual suites. A code or migration commit starts its relevant
workflows. We never treat an existing failed or still-running check as green.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
from urllib.error import URLError, HTTPError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

SHA = re.compile(r"^[a-f0-9]{40}$")
REPO = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
REQUIRED = "Repository rules"
BAD_CONCLUSIONS = {
    "failure", "cancelled", "timed_out", "action_required",
    "startup_failure", "stale",
}
GOOD_CONCLUSIONS = {"success", "neutral", "skipped"}


def evaluate(runs: list[dict], sha: str) -> tuple[str, str]:
    """Return pass/wait/fail and a short explanation; newest rerun wins."""
    latest = {}
    for run in sorted(
        runs, key=lambda item: (item.get("created_at", ""), item.get("id", 0))
    ):
        if run.get("head_sha") != sha or run.get("event") != "push":
            continue
        name = run.get("name")
        if isinstance(name, str):
            latest[name] = run
    if REQUIRED not in latest:
        return "wait", "Repository rules has not started for this commit"
    if not latest:
        return "wait", "No main checks found"
    bad = [
        name for name, run in latest.items()
        if run.get("conclusion") in BAD_CONCLUSIONS
    ]
    if bad:
        return "fail", "Failed checks: " + ", ".join(sorted(bad))
    pending = [
        name for name, run in latest.items()
        if run.get("status") != "completed"
    ]
    if pending:
        return "wait", "Still running: " + ", ".join(sorted(pending))
    unknown = [
        name for name, run in latest.items()
        if run.get("conclusion") not in GOOD_CONCLUSIONS
    ]
    if unknown:
        return "fail", "Unknown check result: " + ", ".join(sorted(unknown))
    if latest[REQUIRED].get("conclusion") != "success":
        return "fail", "Repository rules did not pass"
    return "pass", f"All {len(latest)} applicable checks passed"


def fetch_runs(repo: str, sha: str, token: str) -> list[dict]:
    args = urlencode({"head_sha": sha, "event": "push", "per_page": 100})
    request = Request(
        f"https://api.github.com/repos/{repo}/actions/runs?{args}",
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    with urlopen(request, timeout=15) as response:
        payload = json.load(response)
    if not isinstance(payload, dict) or not isinstance(payload.get("workflow_runs"), list):
        raise ValueError("Unexpected GitHub Actions response")
    if payload.get("total_count", 0) > 100:
        raise ValueError("Too many checks for one SHA; cannot establish complete result")
    return payload["workflow_runs"]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--timeout-seconds", type=int, default=1200)
    opts = parser.parse_args()
    token = os.environ.get("GITHUB_TOKEN", "")
    if not REPO.fullmatch(opts.repo) or opts.repo != "calex794/tor":
        print("TOR_CI_REFUSED: unapproved GitHub repository", file=sys.stderr)
        return 1
    if not SHA.fullmatch(opts.sha) or not token:
        print("TOR_CI_REFUSED: missing token or invalid main SHA", file=sys.stderr)
        return 1
    if not 0 <= opts.timeout_seconds <= 1800:
        print("TOR_CI_REFUSED: timeout outside allowed range", file=sys.stderr)
        return 1
    deadline = time.monotonic() + opts.timeout_seconds
    while True:
        try:
            outcome, detail = evaluate(fetch_runs(opts.repo, opts.sha, token), opts.sha)
        except (ValueError, URLError, HTTPError) as exc:
            print(f"TOR_CI_REFUSED: cannot verify GitHub Actions: {type(exc).__name__}", file=sys.stderr)
            return 1
        print(f"TOR_CI_{outcome.upper()}: {detail}", flush=True)
        if outcome == "pass":
            return 0
        if outcome == "fail" or time.monotonic() >= deadline:
            print("TOR_CI_REFUSED: no verified green build for selected commit", file=sys.stderr)
            return 1
        time.sleep(min(15, max(1, deadline - time.monotonic())))


if __name__ == "__main__":
    raise SystemExit(main())
