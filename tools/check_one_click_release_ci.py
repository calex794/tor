#!/usr/bin/env python3
"""Read-only I-06: verify main ruleset and *every* required push CI job before SSH.

A successful workflow shell is NOT proof that all 14 suites executed. This
script never authorizes production by itself (backup/restore and VPS readiness
must be verified separately).
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from audit_release_candidate import (
    EvidenceError, GitHub, collect, latest_push_runs, require_current_main,
    workflow_names,
)
from release_branch_policy import evaluate_branch_policy

SHA = re.compile(r"^[a-f0-9]{40}$")
REPO = "calex794/tor"


def _fetch_json(repo: str, token: str, path: str, params: dict | None = None):
    url = f"https://api.github.com/repos/{repo}/{path}"
    if params:
        url += "?" + urlencode(params)
    request = Request(url, headers={
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "tor-one-click-release-preflight",
    })
    with urlopen(request, timeout=20) as response:
        return json.load(response)


def fetch_branch(repo: str, token: str) -> dict:
    branch = _fetch_json(repo, token, "branches/main")
    if not isinstance(branch, dict):
        raise ValueError("Malformed main branch metadata")
    return branch


def fetch_rulesets(repo: str, token: str) -> list[dict]:
    # includes_parents ensures organization-level policy is considered.
    summary = _fetch_json(repo, token, "rulesets", {
        "includes_parents": "true", "targets": "branch", "per_page": 100,
    })
    if (not isinstance(summary, list) or len(summary) >= 100
            or any(not isinstance(row, dict) for row in summary)):
        raise ValueError("Incomplete or malformed ruleset list")
    seen: set[int] = set()
    details = []
    for row in summary:
        rid = row.get("id")
        if not isinstance(rid, int) or rid <= 0 or rid in seen:
            raise ValueError("Missing or duplicate ruleset ID")
        seen.add(rid)
        if row.get("target") != "branch":
            raise ValueError("Unexpected ruleset target")
        if row.get("enforcement") != "active":
            continue
        item = _fetch_json(repo, token, f"rulesets/{rid}",
                           {"includes_parents": "true"})
        if (not isinstance(item, dict) or item.get("id") != rid
                or any(item.get(key) != row.get(key)
                       for key in ("target", "source_type", "source",
                                   "enforcement", "updated_at"))):
            raise ValueError("Inconsistent or changing ruleset settings")
        details.append(item)
    return details


def fetch_effective_rules(repo: str, token: str) -> list[dict]:
    # GitHub's effective endpoint returns only ACTIVE branch rules,
    # including the organization's rules. No enabled/evaluate confusion.
    rules = _fetch_json(repo, token, "rules/branches/main", {"per_page": 100})
    if (not isinstance(rules, list) or len(rules) >= 100
            or any(not isinstance(x, dict) for x in rules)):
        raise ValueError("Incomplete or malformed effective branch rules")
    return rules


def verify_policy(repo: str, token: str) -> tuple[str, str]:
    return evaluate_branch_policy(
        fetch_branch(repo, token),
        fetch_rulesets(repo, token),
        fetch_effective_rules(repo, token),
    )


def evaluate(runs: list[dict], sha: str) -> tuple[str, str]:
    """Preliminary wait/fail status; full job evidence is verified by collect()."""
    if not isinstance(runs, list):
        return "fail", "Malformed workflow run list"
    try:
        latest = latest_push_runs(runs, sha)
        expected = workflow_names()
    except (EvidenceError, OSError) as exc:
        return "fail", str(exc)
    unexpected = set(latest) - set(expected)
    if unexpected:
        return "fail", "Unregistered main push workflows: " + ", ".join(sorted(unexpected))
    failing = [name for name, row in latest.items()
               if row.get("status") == "completed" and row.get("conclusion") != "success"]
    if failing:
        return "fail", "Failed mandatory workflows: " + ", ".join(sorted(failing))
    waiting = [name for name in expected
               if name not in latest or latest[name].get("status") != "completed"]
    if waiting:
        return "wait", "Mandatory workflows not yet green: " + ", ".join(sorted(waiting))
    if set(latest) != set(expected):
        return "fail", "Incomplete workflow registry"
    if any(row.get("conclusion") != "success" for row in latest.values()):
        return "fail", "Non-successful workflow"
    return "pass", "Both registered push workflows completed successfully"


def fetch_runs(repo: str, sha: str, token: str) -> list[dict]:
    payload = _fetch_json(repo, token, "actions/runs", {
        "head_sha": sha, "event": "push", "per_page": 100,
    })
    if (not isinstance(payload, dict)
            or not isinstance(payload.get("workflow_runs"), list)
            or not isinstance(payload.get("total_count"), int)
            or payload["total_count"] != len(payload["workflow_runs"])
            or len(payload["workflow_runs"]) > 100):
        raise ValueError("Incomplete GitHub Actions run pagination")
    return payload["workflow_runs"]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--timeout-seconds", type=int, default=1200)
    parser.add_argument("--policy-only", action="store_true",
                        help="read-only verify current SHA and branch protection; no CI/SSH")
    opts = parser.parse_args()
    token = os.environ.get("GITHUB_TOKEN", "")
    if opts.repo != REPO or not SHA.fullmatch(opts.sha) or not token:
        print("TOR_CI_REFUSED: invalid repository, SHA or token", file=sys.stderr)
        return 1
    if not 0 <= opts.timeout_seconds <= 1800:
        print("TOR_CI_REFUSED: timeout outside allowed range", file=sys.stderr)
        return 1

    api = GitHub(token)
    try:
        require_current_main(api, opts.sha)
        state, detail = verify_policy(opts.repo, token)
    except (EvidenceError, ValueError, URLError, HTTPError, TimeoutError) as exc:
        print(f"TOR_CI_REFUSED: cannot verify main rules: {type(exc).__name__}", file=sys.stderr)
        return 1
    if state != "pass":
        print(f"TOR_CI_REFUSED: I-06: {detail}", file=sys.stderr)
        return 1
    if opts.policy_only:
        try:
            require_current_main(api, opts.sha)
        except EvidenceError as exc:
            print(f"TOR_CI_REFUSED: main moved during policy verification: {exc}", file=sys.stderr)
            return 1
        print(f"TOR_CI_POLICY_OK: {detail}")
        return 0

    deadline = time.monotonic() + opts.timeout_seconds
    while True:
        try:
            outcome, detail = evaluate(fetch_runs(opts.repo, opts.sha, token),
                                       opts.sha)
        except (ValueError, URLError, HTTPError, TimeoutError) as exc:
            print(f"TOR_CI_REFUSED: cannot fetch CI: {type(exc).__name__}", file=sys.stderr)
            return 1
        print(f"TOR_CI_{outcome.upper()}: {detail}", flush=True)
        if outcome == "pass":
            # A workflow success can hide missing/skipped suite jobs. Reuse the
            # exact-main, two-workflow/16-job release auditor, fail-closed.
            try:
                report = collect(api, opts.sha)
                if not report["ci_complete"]:
                    print("TOR_CI_REFUSED: incomplete suite evidence: "
                          + "; ".join(report["blockers"]), file=sys.stderr)
                    return 1
                state, detail = verify_policy(opts.repo, token)
                require_current_main(api, opts.sha)
            except (EvidenceError, ValueError, URLError, HTTPError,
                    TimeoutError, OSError) as exc:
                print(f"TOR_CI_REFUSED: final audit incomplete: "
                      f"{type(exc).__name__}", file=sys.stderr)
                return 1
            if state != "pass":
                print(f"TOR_CI_REFUSED: I-06 after CI: {detail}", file=sys.stderr)
                return 1
            print("TOR_CI_VERIFIED: exact-main rules + all 14 suites + ci-required")
            return 0
        if outcome == "fail" or time.monotonic() >= deadline:
            print("TOR_CI_REFUSED: no verified complete green main build", file=sys.stderr)
            return 1
        time.sleep(min(15, max(1, deadline - time.monotonic())))


if __name__ == "__main__":
    raise SystemExit(main())
