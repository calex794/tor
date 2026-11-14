#!/usr/bin/env python3
"""PERF-01: local-only normalization of existing k6 files, not a load generator."""
from __future__ import annotations

import argparse
import csv
import json
import math
from pathlib import Path
import re
import sys

SHA_RE = re.compile(r"[0-9a-f]{40}\Z")
PROFILE_RE = re.compile(r"[a-z][a-z0-9_]{0,31}\Z")
ROUTE_RE = re.compile(r"tor_duration_([a-z][a-z0-9_]{0,31})\Z")


class EvidenceError(ValueError):
    pass


def finite(value, *, whole=False, ratio=False):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    try:
        value = float(value)
    except (OverflowError, ValueError):
        return None
    if not math.isfinite(value) or value < 0 or (ratio and value > 1):
        return None
    if whole:
        return int(value) if value.is_integer() else None
    return round(value, 3)


def metric(data, name, field, **options):
    item = data["metrics"].get(name, {})
    if not isinstance(item, dict):
        return None
    values = item.get("values", item)
    return finite(values.get(field), **options) if isinstance(values, dict) else None


def load_summary(path):
    if not path.is_file() or path.is_symlink() or path.stat().st_size > 8 * 1024 * 1024:
        raise EvidenceError("missing or oversized summary")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (ValueError, UnicodeError, OSError) as exc:
        raise EvidenceError("invalid JSON summary") from exc
    if not isinstance(data, dict) or not isinstance(data.get("metrics"), dict):
        raise EvidenceError("k6 metrics missing")
    routes = {}
    for name in sorted(data["metrics"]):
        match = ROUTE_RE.fullmatch(name)
        if match:
            routes[match.group(1)] = {
                "p50_ms": metric(data, name, "med"),
                "p95_ms": metric(data, name, "p(95)"),
                "p99_ms": metric(data, name, "p(99)"),
            }
    if len(routes) > 32:
        raise EvidenceError("too many route labels")
    return {
        "requests": metric(data, "http_reqs", "count", whole=True),
        "actual_rps": metric(data, "http_reqs", "rate"),
        "http_error_rate": metric(data, "http_req_failed", "rate", ratio=True),
        "dropped_iterations": metric(data, "dropped_iterations", "count", whole=True),
        "p50_ms": metric(data, "http_req_duration", "med"),
        "p95_ms": metric(data, "http_req_duration", "p(95)"),
        "p99_ms": metric(data, "http_req_duration", "p(99)"),
        "route_latency": routes,
    }


def read_results(path, kind):
    if not path.exists():
        return []
    if not path.is_file() or path.is_symlink() or path.stat().st_size > 8192:
        raise EvidenceError("invalid results file")
    output, seen = [], set()
    lines = path.read_text(encoding="utf-8").splitlines()
    if len(lines) > 101:
        raise EvidenceError("too many result rows")
    for idx, cells in enumerate(csv.reader(lines, delimiter="\t")):
        if not cells:
            continue
        if kind == "diagnostic" and idx == 0 and cells == ["profile", "target_rps", "outcome"]:
            continue
        if kind == "base" and len(cells) == 2:
            profile, rate, status = "mixed", *cells
        elif kind == "diagnostic" and len(cells) == 3:
            profile, rate, status = cells
        else:
            raise EvidenceError("invalid result columns")
        if (not PROFILE_RE.fullmatch(profile) or not rate.isdecimal()
                or int(rate) not in (25, 50, 100, 200, 250, 300, 350, 400, 500)
                or status not in ("PASS", "FAIL", "THRESHOLD_OR_RUNTIME_FAIL")):
            raise EvidenceError("invalid result values")
        identity = (profile, int(rate))
        if identity in seen:
            raise EvidenceError("duplicate stage")
        seen.add(identity)
        output.append({"group": kind, "profile": profile, "target_rps": int(rate),
                       "k6_exit": "PASS" if status == "PASS" else "FAIL"})
    return output


def stage(root, entry):
    path = (root / f"summary-{entry['target_rps']}.json"
            if entry["group"] == "base" else
            root / "diagnostic" / f"summary-{entry['profile']}-{entry['target_rps']}.json")
    output = dict(entry)
    try:
        output.update(load_summary(path))
        keys = ("requests", "actual_rps", "http_error_rate", "dropped_iterations",
                "p50_ms", "p95_ms", "p99_ms")
        complete = all(output[k] is not None for k in keys)
    except EvidenceError:
        complete = False
        output.update({k: None for k in ("requests", "actual_rps", "http_error_rate",
                                         "dropped_iterations", "p50_ms", "p95_ms", "p99_ms")})
        output["route_latency"] = {}
    output["evidence"] = ("INCOMPLETE" if not complete else
                          "PASS" if output["k6_exit"] == "PASS" else "FAIL")
    output["sqlx_queries_per_request"] = None
    output["sqlx_time_per_request_ms"] = None
    return output


def build(root, sha, run_id=None):
    if not SHA_RE.fullmatch(sha):
        raise EvidenceError("full source SHA required")
    if run_id is not None and (not run_id.isdecimal() or len(run_id) > 20):
        raise EvidenceError("numeric GitHub run ID required")
    if not root.is_dir():
        raise EvidenceError("isolated benchmark directory missing")
    source = read_results(root / "results.tsv", "base")
    source += read_results(root / "diagnostic/results.tsv", "diagnostic")
    if len(source) > 100:
        raise EvidenceError("too many stages")
    stages = [stage(root, item) for item in source]
    supplemental = {}
    for label, filename in (("reader_journeys", "journeys/summary.json"),
                            ("authenticated_writes", "progress/summary.json")):
        try:
            supplemental[label] = load_summary(root / filename)
        except EvidenceError:
            supplemental[label] = None
    cpu = None
    cpu_file = root / "cpu_count.txt"
    if cpu_file.is_file() and not cpu_file.is_symlink() and cpu_file.stat().st_size < 20:
        count = cpu_file.read_text(encoding="ascii").strip()
        if count.isdecimal() and 0 < int(count) <= 4096:
            cpu = int(count)
    return {
        "schema_version": 1, "source_sha": sha, "github_run_id": run_id,
        "environment": {"runner": "isolated ubuntu-24.04 GitHub-hosted",
                        "database": "disposable PostgreSQL 18 / tor_ci",
                        "target": "http://127.0.0.1:3089",
                        "runner_logical_cpus": cpu, "generator_colocated": True},
        "report_state": ("MEASURED" if stages and
                         all(x["evidence"] != "INCOMPLETE" for x in stages) else "INCOMPLETE"),
        "stages": stages, "supplemental": supplemental,
        "unmeasured": {"sqlx_queries_per_http_request": None,
                       "sqlx_time_per_http_request_ms": None,
                       "postgresql_server_cpu_ms": None,
                       "postgresql_wait_ms_per_request": None,
                       "vps_capacity": None},
        "raw_evidence_present": {
            "process_cpu_rss_io": (root / "diagnostic/pidstat.txt").is_file(),
            "postgresql_wait_samples": (root / "diagnostic/pg-waits.tsv").is_file(),
            "vmstat": (root / "diagnostic/vmstat.txt").is_file(),
        },
        "caveats": [
            "HTTP latency is not PostgreSQL CPU time.",
            "SQLx operations per route are unmeasured by this harness.",
            "Missing measurements are null, never observed zero.",
            "MEASURED denotes complete artifacts, not a performance PASS.",
            "k6 and TOR share a hosted runner, not the production VPS.",
        ],
    }


def markdown(report):
    def val(x):
        return "—" if x is None else str(x)
    lines = [
        "## PERF-01 — existing isolated k6 evidence",
        "Source SHA: " + report["source_sha"] + "; state: " + report["report_state"],
        "",
        "| Group | Profile | Target RPS | Actual RPS | p50/p95/p99 ms | Error rate | Dropped | Evidence |",
        "|---|---|---:|---:|---|---:|---:|---|",
    ]
    for row in report["stages"]:
        latency = "/".join(val(row[k]) for k in ("p50_ms", "p95_ms", "p99_ms"))
        lines.append("| {} | {} | {} | {} | {} | {} | {} | {} |".format(
            row["group"], row["profile"], row["target_rps"],
            val(row["actual_rps"]), latency, val(row["http_error_rate"]),
            val(row["dropped_iterations"]), row["evidence"]))
    if not report["stages"]:
        lines.append("| — | — | — | — | — | — | — | INCOMPLETE |")
    lines += [
        "",
        "SQLx queries/request, SQLx time/request and PostgreSQL CPU are **unmeasured** "
        "(null), not zero. Failed high-rate stages are retained; absent summaries "
        "are INCOMPLETE. k6 and TOR share a GitHub runner, not the VPS.",
        "",
    ]
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--run-id")
    parser.add_argument("--json", type=Path, required=True)
    parser.add_argument("--markdown", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        report = build(args.root, args.sha, args.run_id)
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False) + "\n",
                             encoding="utf-8")
        args.markdown.parent.mkdir(parents=True, exist_ok=True)
        args.markdown.write_text(markdown(report), encoding="utf-8")
    except (EvidenceError, OSError, UnicodeError) as exc:
        print("PERF01_REPORT_UNAVAILABLE: " + str(exc), file=sys.stderr)
        return 2
    print("TOR_PERF01_REPORT_{} stages={}".format(report["report_state"], len(report["stages"])))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
