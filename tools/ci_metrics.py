#!/usr/bin/env python3
"""Measure actual GitHub Actions queue/run times for one TOR commit.

Read-only: requires no third-party packages, writes no GitHub state. All totals
are observed job wall-clock times, *not* GitHub billing/invoice minutes.
"""
from __future__ import annotations

import argparse
from datetime import datetime
import json
import math
import os
import re
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

REPO_PATTERN = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\Z")
SHA_PATTERN = re.compile(r"[0-9a-fA-F]{40}\Z")


class MetricsError(Exception):
    pass


class GitHub:
    def __init__(self, token=None):
        self.token = token or None

    def get(self, path, query=None):
        url = "https://api.github.com" + path
        if query:
            url += "?" + urlencode(query)
        headers = {
            "Accept": "application/vnd.github+json",
            "User-Agent": "tor-ci-metrics/1.0",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        try:
            with urlopen(Request(url, headers=headers), timeout=20) as response:
                return json.load(response)
        except HTTPError as exc:
            # Never echo the Authorization header or URL query to diagnostics.
            raise MetricsError(f"GitHub API HTTP {exc.code} at {path}") from exc
        except (URLError, TimeoutError, ValueError) as exc:
            raise MetricsError(f"Cannot read GitHub API response at {path}: {type(exc).__name__}") from exc


def timestamp(raw):
    if not raw:
        return None
    try:
        value = datetime.fromisoformat(raw.replace("Z", "+00:00"))
        if value.utcoffset() is None:
            raise ValueError("missing timezone")
        return value
    except (TypeError, ValueError) as exc:
        raise MetricsError("Invalid GitHub Actions timestamp") from exc


def elapsed_seconds(start, end):
    if start is None or end is None:
        return None
    seconds = (end - start).total_seconds()
    if seconds < 0:
        raise MetricsError("Job timestamps are out of order")
    return seconds


def percentile(values, ratio):
    if not values:
        return None
    ordered = sorted(values)
    pos = (len(ordered) - 1) * ratio
    lo, hi = math.floor(pos), math.ceil(pos)
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (pos - lo)


def paginated(api, path, field, max_items):
    out = []
    for page in range(1, 21):
        data = api.get(path, {"per_page": 100, "page": page})
        values = data.get(field)
        if not isinstance(values, list):
            raise MetricsError(f"GitHub API response misses {field}")
        out.extend(values)
        if len(out) > max_items:
            raise MetricsError(f"More than {max_items} {field}; raise --max-runs/--max-jobs")
        count = data.get("total_count")
        if count is not None and len(out) >= count:
            return out
        if len(values) < 100:
            if count is not None and len(out) < count:
                raise MetricsError(f"Incomplete GitHub {field} pagination")
            return out
    raise MetricsError(f"GitHub {field} pagination exceeds 20 pages")


def resolve_sha(api, repo, supplied):
    if supplied:
        if not SHA_PATTERN.fullmatch(supplied):
            raise MetricsError("--sha and --before-sha must be full 40-character commit IDs")
        return supplied.lower()
    info = api.get(f"/repos/{repo}")
    branch = info.get("default_branch")
    if not branch or not re.fullmatch(r"[A-Za-z0-9_.\-/]+", branch):
        raise MetricsError("Cannot determine repository default branch")
    return api.get(f"/repos/{repo}/branches/{branch}")["commit"]["sha"].lower()


def find_runs(api, repo, sha, limit):
    # GitHub's head_sha server-side filter is required: never compare an
    # accidental mixture of recent branch commits as if it were one build.
    path = f"/repos/{repo}/actions/runs"
    out = []
    for page in range(1, 21):
        data = api.get(path, {"head_sha": sha, "per_page": 100, "page": page})
        values = data.get("workflow_runs")
        if not isinstance(values, list):
            raise MetricsError("GitHub Actions workflow list is unavailable")
        out.extend(values)
        if len(out) > limit:
            raise MetricsError(f"More than {limit} runs for {sha[:12]}; use --max-runs")
        count = data.get("total_count")
        if count is not None and len(out) >= count:
            break
        if len(values) < 100:
            if count is not None and len(out) < count:
                raise MetricsError("Incomplete workflow-run pagination")
            break
    else:
        raise MetricsError("Workflow-run pagination exceeds 20 pages")
    return out


def load_group(api, repo, label, sha=None, run_ids=(), max_runs=50, max_jobs=1500):
    if run_ids:
        if len(set(run_ids)) != len(run_ids):
            raise MetricsError("Duplicate run IDs would double-count jobs")
        runs = [api.get(f"/repos/{repo}/actions/runs/{rid}") for rid in run_ids]
    else:
        sha = resolve_sha(api, repo, sha)
        runs = find_runs(api, repo, sha, max_runs)
    if len(runs) > max_runs:
        raise MetricsError(f"More than {max_runs} runs in {label}; use --max-runs")
    if not runs:
        raise MetricsError(f"No workflow runs for {label}")
    shas = {run.get("head_sha", "").lower() for run in runs}
    if len(shas) != 1 or not SHA_PATTERN.fullmatch(next(iter(shas))):
        raise MetricsError(f"{label} mixes commits or contains invalid SHA")
    found_sha = next(iter(shas))
    if sha and found_sha != sha.lower():
        raise MetricsError(f"{label}: run SHA does not match --sha")
    ids = [run.get("id") for run in runs]
    if len(set(ids)) != len(ids):
        raise MetricsError("Duplicate workflow IDs would double-count jobs")
    queue_times, run_times = [], []
    jobs_started = jobs_done = jobs_skipped = jobs_waiting = 0
    outcomes = []
    complete = True
    workflow_names, events, executed_jobs, skipped_job_names = set(), set(), set(), set()
    for run in runs:
        workflow, event = run.get("name"), run.get("event")
        if not isinstance(workflow, str) or not workflow.strip():
            raise MetricsError("Run has no workflow name")
        if workflow in workflow_names:
            raise MetricsError("Repeated workflow on one SHA would double-count its jobs")
        workflow_names.add(workflow)
        if not isinstance(event, str) or not event.strip():
            raise MetricsError("Run has no GitHub event type")
        events.add(event)
        if len(events) != 1:
            raise MetricsError("Different GitHub event types cannot share one CI measurement")
        status, conclusion = run.get("status"), run.get("conclusion")
        if status != "completed" or conclusion != "success":
            complete = False  # failed/cancelled workflows cannot prove a speedup
        outcomes.append({"run_id": run["id"], "workflow": run.get("name"), "status": status,
                         "conclusion": conclusion, "url": run.get("html_url")})
        jobs = paginated(api, f"/repos/{repo}/actions/runs/{run['id']}/jobs", "jobs", max_jobs)
        for job in jobs:
            if job.get("run_id") not in (None, run["id"]):
                raise MetricsError("Job belongs to a different workflow run")
            job_name = job.get("name")
            if not isinstance(job_name, str) or not job_name.strip():
                raise MetricsError("Job has no name; cannot establish comparable test coverage")
            signature = f"{workflow} / {job_name}"
            if signature in executed_jobs or signature in skipped_job_names:
                raise MetricsError("Duplicate job name in a workflow")
            start, created, end = (timestamp(job.get(k)) for k in
                                   ("started_at", "created_at", "completed_at"))
            if job.get("conclusion") == "skipped":
                jobs_skipped += 1
                skipped_job_names.add(signature)
                continue
            if job.get("conclusion") != "success":
                complete = False  # pending/failed work cannot prove speedup
            if start is None:
                jobs_waiting += 1
                complete = False
                continue
            jobs_started += 1
            executed_jobs.add(signature)
            queue = elapsed_seconds(created, start)
            if queue is None:
                complete = False
            else:
                queue_times.append(queue)
            elapsed = elapsed_seconds(start, end)
            if elapsed is None:
                complete = False
            else:
                jobs_done += 1
                run_times.append(elapsed)
    return {
        "label": label,
        "sha": found_sha,
        "workflow_runs": len(runs),
        "event": next(iter(events)),
        "workflow_names": sorted(workflow_names),
        "executed_jobs": sorted(executed_jobs),
        "skipped_jobs": sorted(skipped_job_names),
        "jobs_started": jobs_started,
        "jobs_completed": jobs_done,
        "jobs_skipped": jobs_skipped,
        "jobs_not_started": jobs_waiting,
        "queue_p50_seconds": percentile(queue_times, 0.50),
        "queue_p95_seconds": percentile(queue_times, 0.95),
        "run_p50_seconds": percentile(run_times, 0.50),
        "run_p95_seconds": percentile(run_times, 0.95),
        "runner_minutes": sum(run_times) / 60,
        "complete": complete and bool(run_times) and bool(queue_times),
        "runs": outcomes,
    }


def comparison_blocker(before, after):
    """Only comparable executed tests can support an I-09 speed comparison.

    A faster run after skipping one or more checks is not a performance gain
    with equivalent coverage. The raw measurements remain available.
    """
    if not before["complete"] or not after["complete"]:
        return "unfinished or unsuccessful workflow/job evidence"
    if before["event"] != after["event"]:
        return "mixed events (push, pull_request or manual workflow)"
    if before["workflow_names"] != after["workflow_names"]:
        return "different workflow sets"
    if before["skipped_jobs"] or after["skipped_jobs"]:
        return "skipped jobs mean the required coverage was not demonstrated"
    if before["executed_jobs"] != after["executed_jobs"]:
        return "different executed job sets"
    return None


def difference(before, after):
    keys = ("jobs_started", "queue_p50_seconds", "queue_p95_seconds", "run_p50_seconds",
            "run_p95_seconds", "runner_minutes")
    return {key: {"before": before[key], "after": after[key],
                  "delta": after[key] - before[key],
                  "percent": ((after[key] / before[key] - 1) * 100 if before[key] else None)}
            for key in keys}


def pretty(group):
    def fmt(k, unit="s"):
        value = group[k]
        return "n/a" if value is None else f"{value:.1f}{unit}"
    return (f"{group['label']}: {group['sha']}\n"
            f"  Workflows: {group['workflow_runs']}; jobs: {group['jobs_started']} started, "
            f"{group['jobs_completed']} completed, {group['jobs_skipped']} skipped, "
            f"{group['jobs_not_started']} waiting\n"
            f"  Queue p50/p95: {fmt('queue_p50_seconds')} / {fmt('queue_p95_seconds')}; "
            f"job run p50/p95: {fmt('run_p50_seconds')} / {fmt('run_p95_seconds')}\n"
            f"  Runner-minutes (wall-clock estimate, not billing): "
            f"{fmt('runner_minutes', 'm')}; complete: {group['complete']}")


def main(argv=None, api=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=os.getenv("GITHUB_REPOSITORY", "calex794/tor"))
    parser.add_argument("--sha", help="Target full 40-character SHA (default: current default branch)")
    parser.add_argument("--run", action="append", type=int, default=[], metavar="ID",
                        help="Run ID on target SHA; repeat for all workflows")
    parser.add_argument("--before-sha", help="Baseline full SHA for comparison")
    parser.add_argument("--before-run", action="append", type=int, default=[], metavar="ID",
                        help="Baseline run ID; repeat for all workflows")
    parser.add_argument("--max-runs", type=int, default=50)
    parser.add_argument("--max-jobs", type=int, default=1500)
    parser.add_argument("--report", action="store_true", help="Print human-readable report (default)")
    parser.add_argument("--json", action="store_true", help="Print machine-readable JSON instead")
    opts = parser.parse_args(argv)
    if not REPO_PATTERN.fullmatch(opts.repo) or not (1 <= opts.max_runs <= 200) or not (1 <= opts.max_jobs <= 2000):
        parser.error("Invalid repo or bounds")
    for rid in opts.run + opts.before_run:
        if rid <= 0:
            parser.error("Run ID must be positive")
    if (opts.sha and not SHA_PATTERN.fullmatch(opts.sha)) or (opts.before_sha and not SHA_PATTERN.fullmatch(opts.before_sha)):
        parser.error("Commit SHA must contain exactly 40 hex characters")
    api = api or GitHub(os.getenv("GITHUB_TOKEN") or os.getenv("GH_TOKEN"))
    try:
        target = load_group(api, opts.repo, "target", opts.sha, opts.run, opts.max_runs, opts.max_jobs)
        baseline = None
        if opts.before_sha or opts.before_run:
            baseline = load_group(api, opts.repo, "baseline", opts.before_sha,
                                  opts.before_run, opts.max_runs, opts.max_jobs)
        blocker = comparison_blocker(baseline, target) if baseline else None
        compared = difference(baseline, target) if baseline and blocker is None else None
        data = {"repository": opts.repo, "target": target, "baseline": baseline,
                "comparison": compared, "comparison_blocker": blocker,
                "method": "matched executed jobs and timestamps; not billing or statistical proof"}
        if opts.json:
            print(json.dumps(data, ensure_ascii=False, indent=2))
        else:
            print(pretty(target))
            if baseline:
                print(pretty(baseline))
                if compared:
                    print("Comparison (target - baseline):")
                    for k, v in compared.items():
                        print(f"  {k}: {v['delta']:+.2f} ({v['percent']:+.1f}% change)" if v["percent"] is not None else f"  {k}: {v['delta']:+.2f}")
                else:
                    print(f"Comparison suppressed: {blocker}")
            print("Note: runner-minutes sum observed job durations; no billing multiplier or rounding.")
        if baseline and compared is None:
            return 2
        return 0 if target["complete"] else 2
    except MetricsError as exc:
        print(f"ci_metrics: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
