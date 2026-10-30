#!/usr/bin/env python3
"""I-12: read-only CI evidence for an exact TOR main release candidate.

This is NOT a deployment approval: product parity, operator controls, external
payments, backup restoration and real host health require separate evidence.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from ci_test_map import CI_WORKFLOW, SUITES, SUITE_JOB_IDS

ROOT = Path(__file__).resolve().parents[1]
SHA = re.compile(r"[0-9a-f]{40}\Z")
REPO = "calex794/tor"


class EvidenceError(ValueError):
    """Incomplete, contradictory or unsafe evidence; never assume success."""


def workflow_names(root: Path = ROOT) -> dict[str, str]:
    """Resolve the registered automatic push workflows, not manual suite wrappers."""
    directory = root / ".github" / "workflows"
    names: dict[str, str] = {}
    for filename in ("rules.yml", CI_WORKFLOW):
        source = (directory / filename).read_text(encoding="utf-8")
        first = source.splitlines()[0].strip() if source else ""
        if not first.startswith("name: "):
            raise EvidenceError(f"missing workflow title: {filename}")
        title = first.removeprefix("name: ").strip().strip(chr(34) + chr(39))
        if not title or title in names:
            raise EvidenceError(f"duplicate or empty workflow title: {filename}")
        names[title] = filename
    if len(names) != 2:
        raise EvidenceError("required automatic CI workflows have duplicate entries")
    return names


def latest_push_runs(runs: list[dict], sha: str) -> dict[str, dict]:
    """Use the latest retry only, filtering to the exact push SHA."""
    result: dict[str, dict] = {}
    for item in runs:
        if not isinstance(item, dict):
            raise EvidenceError("non-object workflow run")
        if item.get("event") != "push" or item.get("head_sha") != sha:
            continue
        name, run_id = item.get("name"), item.get("id")
        if not isinstance(name, str) or not isinstance(run_id, int) or run_id <= 0:
            raise EvidenceError("push run without name or numeric ID")
        attempt = item.get("run_attempt", 1)
        if not isinstance(attempt, int) or attempt < 1:
            raise EvidenceError("bad workflow attempt")
        rank = (str(item.get("created_at") or ""), attempt, run_id)
        existing = result.get(name)
        if existing is None:
            result[name] = item
        else:
            old_rank = (str(existing.get("created_at") or ""),
                        existing.get("run_attempt", 1), existing["id"])
            if rank > old_rank:
                result[name] = item
    return result


def is_selector(job: dict) -> bool:
    """The reusable suite selector is not evidence that tests executed."""
    name = job.get("name")
    return isinstance(name, str) and (name == "select" or name.startswith("select / "))


def evaluate_ci(
    runs: list[dict],
    jobs_by_run: dict[int, list[dict]],
    sha: str,
    expected: dict[str, str],
) -> dict:
    if not SHA.fullmatch(sha):
        raise EvidenceError("candidate SHA must be 40 lowercase hex characters")
    if (not isinstance(expected, dict)
            or set(expected.values()) != {"rules.yml", CI_WORKFLOW}
            or len(expected) != 2):
        raise EvidenceError("incomplete or unreviewed automatic workflow registry")
    if len(SUITE_JOB_IDS) != len(SUITES) or len(set(SUITE_JOB_IDS.values())) != len(SUITES):
        raise EvidenceError("duplicate or incomplete suite-job registry")
    current = latest_push_runs(runs, sha)
    blockers: list[str] = []
    verified: list[str] = []
    checks: dict[str, dict] = {}
    suite_checks = {
        suite: {"job": job_id, "state": "missing"}
        for suite, job_id in SUITE_JOB_IDS.items()
    }
    for title, filename in sorted(expected.items()):
        run = current.get(title)
        if run is None:
            blockers.append(f"{title}: missing push workflow")
            checks[title] = {"workflow": filename, "state": "missing"}
            continue
        run_id = run["id"]
        details = {"workflow": filename, "run_id": run_id}
        if run.get("status") != "completed" or run.get("conclusion") != "success":
            blockers.append(f"{title}: workflow not successfully completed")
            checks[title] = dict(details, state="blocked")
            continue
        jobs = jobs_by_run.get(run_id)
        if not isinstance(jobs, list) or not jobs:
            blockers.append(f"{title}: job evidence missing")
            checks[title] = dict(details, state="missing_jobs")
            continue
        if any(not isinstance(job, dict) for job in jobs):
            raise EvidenceError("non-object job")
        names = [job.get("name") for job in jobs]
        if any(not isinstance(name, str) or not name for name in names):
            raise EvidenceError("missing job name")
        if len(names) != len(set(names)):
            blockers.append(f"{title}: duplicate job evidence")
            checks[title] = dict(details, state="duplicate_jobs")
            continue
        by_name = dict(zip(names, jobs))
        if filename == "rules.yml":
            required = {"rules"}
        else:
            selectors = {name for name in names if is_selector({"name": name})}
            if len(selectors) != 1 or not selectors <= {"select", "select / select"}:
                blockers.append(f"{title}: exactly one successful selector is required")
                checks[title] = dict(details, state="invalid_selector")
                continue
            required = set(SUITE_JOB_IDS.values()) | selectors
            for suite, job_id in SUITE_JOB_IDS.items():
                job = by_name.get(job_id)
                if job is not None:
                    state = ("verified" if job.get("status") == "completed"
                             and job.get("conclusion") == "success" else "blocked")
                    suite_checks[suite] = {"job": job_id, "state": state}
        missing, unexpected = sorted(required - set(names)), sorted(set(names) - required)
        if missing or unexpected:
            if filename == CI_WORKFLOW and len(names) == 1 and is_selector(jobs[0]):
                blockers.append(f"{title}: selector-only success is not a test")
            else:
                blockers.append(f"{title}: missing/unknown jobs: {missing} / {unexpected}")
            checks[title] = dict(details, state="invalid_jobs",
                                 missing=missing, unexpected=unexpected)
            continue
        # Every required job, including the planner, must succeed on this SHA.
        if any(job.get("status") != "completed" or job.get("conclusion") != "success"
               for job in jobs):
            blockers.append(f"{title}: a job failed, was skipped or is pending")
            checks[title] = dict(details, state="blocked_jobs")
            continue
        verified.append(title)
        checks[title] = dict(details, state="verified", jobs=len(jobs))
    for title in sorted(set(current) - set(expected)):
        blockers.append(f"{title}: unregistered push workflow")
    return {
        "schema": "tor.release_ci_evidence.v2",
        "sha": sha,
        "scope": "CI-only, not a production or product-parity approval",
        "ci_complete": not blockers,
        "release_approved": False,
        "verified_workflows": len(verified),
        "required_workflows": len(expected),
        "verified_suites": sum(x["state"] == "verified" for x in suite_checks.values()),
        "required_suites": len(SUITE_JOB_IDS),
        "checks": checks,
        "suite_checks": suite_checks,
        "blockers": blockers,
        "external_gates": [
            "I-06: enforce branch protection administratively",
            "I-10: isolated staging HTTPS and PostgreSQL evidence",
            "I-11: backup restore and operator rollback on a real test host",
            "I-12: REL-01..10, 61 accepted scenarios and external money/ACL gates",
            "I-13: Notion per-scenario evidence matched to released SHA",
        ],
    }


class GitHub:
    def __init__(self, token: str):
        if not token:
            raise EvidenceError("missing GITHUB_TOKEN (read-only Actions access)")
        self.token = token

    def get(self, path: str, params: dict | None = None) -> dict:
        url = "https://api.github.com" + path
        if params:
            url += "?" + urlencode(params)
        request = Request(url, headers={
            "Accept": "application/vnd.github+json",
            "Authorization": "Bearer " + self.token,
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "tor-release-ci-evidence",
        })
        try:
            with urlopen(request, timeout=20) as response:
                value = json.load(response)
        except HTTPError as exc:
            raise EvidenceError(f"GitHub API HTTP {exc.code}; no CI approval") from exc
        except (URLError, TimeoutError, ValueError) as exc:
            raise EvidenceError("GitHub API unavailable or malformed; no CI approval") from exc
        if not isinstance(value, dict):
            raise EvidenceError("GitHub API returned non-object")
        return value


def require_current_main(api: GitHub, sha: str) -> None:
    ref = api.get(f"/repos/{REPO}/git/ref/heads/main")
    obj = ref.get("object")
    if (ref.get("ref") != "refs/heads/main"
            or not isinstance(obj, dict) or obj.get("sha") != sha):
        raise EvidenceError("candidate is not the current main SHA")


def collect(api: GitHub, sha: str, root: Path = ROOT) -> dict:
    if not SHA.fullmatch(sha):
        raise EvidenceError("invalid release candidate SHA")
    # A frequently updated main can move while required job evidence is fetched.
    require_current_main(api, sha)
    payload = api.get(f"/repos/{REPO}/actions/runs",
                      {"head_sha": sha, "event": "push", "per_page": 100})
    runs = payload.get("workflow_runs")
    if (not isinstance(runs, list) or not isinstance(payload.get("total_count"), int)
            or payload["total_count"] != len(runs) or len(runs) > 100):
        raise EvidenceError("incomplete GitHub workflow pagination")
    expected = workflow_names(root)
    latest = latest_push_runs(runs, sha)
    jobs: dict[int, list[dict]] = {}
    for title in set(latest) & set(expected):
        record = latest[title]
        if record.get("status") != "completed" or record.get("conclusion") != "success":
            continue
        response = api.get(f"/repos/{REPO}/actions/runs/{record['id']}/jobs",
                           {"per_page": 100})
        values = response.get("jobs")
        if (not isinstance(values, list) or not isinstance(response.get("total_count"), int)
                or response["total_count"] != len(values) or len(values) > 100):
            raise EvidenceError("incomplete GitHub job pagination")
        jobs[record["id"]] = values
    require_current_main(api, sha)
    return evaluate_ci(runs, jobs, sha, expected)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sha", required=True, help="exact 40-character current main SHA")
    parser.add_argument("--repo", default=REPO)
    parser.add_argument("--out", help="write JSON report to this path")
    parser.add_argument("--strict", action="store_true",
                        help="fail if any CI evidence is incomplete")
    options = parser.parse_args()
    try:
        if options.repo != REPO:
            raise EvidenceError("unapproved repository")
        result = collect(GitHub(os.environ.get("GITHUB_TOKEN", "")), options.sha)
        output = json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
        if options.out:
            Path(options.out).write_text(output, encoding="utf-8")
        else:
            sys.stdout.write(output)
        return 1 if options.strict and not result["ci_complete"] else 0
    except (EvidenceError, OSError) as exc:
        print(f"TOR_RELEASE_EVIDENCE_BLOCKED: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
