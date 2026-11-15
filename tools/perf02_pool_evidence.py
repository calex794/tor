#!/usr/bin/env python3
"""PERF-02g: summarize numeric SQLx pool checkout times in disposable load CI.

Acquire duration includes queueing, liveness checks and new connections.
It is not PostgreSQL CPU, database lock wait, or pure pool queue time.
Never open a live database; never copy a bearer, query, URL, or trace identifier.
"""
import argparse
import csv
from datetime import datetime
import json
import math
from pathlib import Path
import re

LINE = re.compile(
    r"^TOR_PERF02_POOL_SAMPLE epoch_ms=(\d{13}) status=(\d{3}) "
    r"http_us=(\d+) queries=(\d+) acquires=(\d+) "
    r"acquire_total_us=(\d+) acquire_max_us=(\d+)$"
)
ALLOWED_RATES = (20, 50, 100, 200)
MAX_INPUT_BYTES = 16 * 1024 * 1024


class EvidenceError(ValueError):
    pass


def read_file(path):
    if not path.is_file() or path.is_symlink() or path.stat().st_size > MAX_INPUT_BYTES:
        raise EvidenceError("missing, linked or oversized PERF-02 file")
    return path.read_text(encoding="utf-8")


def ms_timestamp(value):
    try:
        point = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise EvidenceError("invalid stage timestamp") from exc
    if point.utcoffset() is None:
        raise EvidenceError("stage timezone missing")
    return int(point.timestamp() * 1000)


def percentile(values, p):
    if not values:
        return None
    return sorted(values)[math.ceil(len(values) * p) - 1]


def stage_windows(root):
    stage_lines = read_file(root / "stages.tsv").splitlines()
    windows = {}
    for row in csv.reader(stage_lines, delimiter="\t"):
        if len(row) != 3 or row[2] not in ("start", "finish") or not row[1].isdecimal():
            raise EvidenceError("malformed stage boundaries")
        rate = int(row[1])
        if rate not in ALLOWED_RATES:
            raise EvidenceError("unsupported stage rate")
        key = (rate, row[2])
        if key in windows:
            raise EvidenceError("duplicate stage boundary")
        windows[key] = ms_timestamp(row[0])
    if len(windows) != 2 * len(ALLOWED_RATES):
        raise EvidenceError("stage boundaries incomplete")
    output = {}
    for rate in ALLOWED_RATES:
        begin, end = windows[(rate, "start")], windows[(rate, "finish")]
        if begin >= end:
            raise EvidenceError("stage boundaries reversed")
        output[rate] = (begin, end)  # CI shell records millisecond boundaries
    return output


def result_status(root):
    rows = list(csv.reader(read_file(root / "results.tsv").splitlines(), delimiter="\t"))
    if not rows or rows[0] != ["target_rps", "outcome"]:
        raise EvidenceError("unexpected result file header")
    if len(rows) != len(ALLOWED_RATES) + 1:
        raise EvidenceError("result rows incomplete")
    found = {}
    for row in rows[1:]:
        if len(row) != 2 or not row[0].isdecimal() or row[1] not in ("PASS", "FAIL"):
            raise EvidenceError("invalid stage result")
        rate = int(row[0])
        if rate in found or rate not in ALLOWED_RATES:
            raise EvidenceError("duplicate or unexpected stage")
        found[rate] = row[1]
    return found


def numeric_samples(root):
    observed = []
    for line in read_file(root / "server.log").splitlines():
        if not line.startswith("TOR_PERF02_POOL_SAMPLE"):
            continue
        match = LINE.fullmatch(line)
        if not match:
            raise EvidenceError("malformed numeric pool sample")
        epoch_ms, status, http_us, queries, acquires, total, maximum = map(int, match.groups())
        if status < 100 or status > 599 or maximum > total or (
            acquires == 0 and (total != 0 or maximum != 0)
        ):
            raise EvidenceError("invalid pool acquisition values")
        observed.append({
            "epoch_ms": epoch_ms,
            "status": status,
            "http_us": http_us,
            "queries": queries,
            "acquires": acquires,
            "acquire_total_us": total,
            "acquire_max_us": maximum,
        })
        if len(observed) > 100_000:
            raise EvidenceError("unbounded pool sample count")
    return observed


def k6_latency(root, rate):
    path = root / f"summary-{rate}.json"
    try:
        data = json.loads(read_file(path))
        metrics = data["metrics"]
        def view(name):
            metric = metrics[name]
            # k6 --summary-export writes trend stats directly under the metric.
            # Older/custom exporters may wrap the same numbers in "values".
            values = metric.get("values", metric)
            p50, p95, p99 = (values["med"], values["p(95)"], values["p(99)"])
            if not all(isinstance(x, (int, float)) and
                       not isinstance(x, bool) and math.isfinite(x) and x >= 0
                       for x in (p50, p95, p99)):
                raise EvidenceError("invalid k6 latency values")
            return {"p50_ms": round(p50, 3),
                    "p95_ms": round(p95, 3),
                    "p99_ms": round(p99, 3)}
        return {
            "guest": view("perf02_book_guest_ms"),
            "reader": view("perf02_book_reader_ms"),
        }
    except (KeyError, TypeError, json.JSONDecodeError) as exc:
        raise EvidenceError("k6 tagged latency metrics missing") from exc


def summarize(root):
    windows = stage_windows(root)
    outcomes = result_status(root)
    samples = numeric_samples(root)
    stages = []
    for rate in ALLOWED_RATES:
        begin, end = windows[rate]
        # Approximately one in sixteen Book GETs are hash-sampled. Requests
        # from the fixture setup can also appear within the stage boundary.
        rows = [x for x in samples if begin <= x["epoch_ms"] <= end]
        valid = [x for x in rows if x["acquires"] > 0]
        if outcomes[rate] == "PASS" and (len(rows) < 4 or len(valid) < 4):
            raise EvidenceError(
                f"no trustworthy SQLx pool timing for passing {rate} GET/s stage"
            )
        def p95(field):
            return percentile([v[field] for v in valid], 0.95)
        try:
            latency = k6_latency(root, rate)
        except EvidenceError:
            if outcomes[rate] == "PASS":
                raise
            latency = None  # failed higher-rate setup is never zero latency
        # A sampling cadence synchronized with alternating k6 scenarios can
        # accidentally select only guests or only readers. Preserve the
        # numerical SQLx-query-count distribution to expose this bias without
        # recording user/session identities or guessing their roles.
        cohorts = {}
        for query_count in sorted({v["queries"] for v in valid}):
            subset = [v for v in valid if v["queries"] == query_count]
            cohorts[str(query_count)] = {
                "samples": len(subset),
                "acquire_max_us_p95": percentile(
                    [v["acquire_max_us"] for v in subset], 0.95
                ),
            }
        stages.append({
            "target_get_per_s": rate,
            "k6_outcome": outcomes[rate],
            "k6_latency": latency,
            "sampled_book_requests": len(rows),
            "with_successful_acquire": len(valid),
            "by_observed_sqlx_query_count": cohorts,
            "acquires_per_request_p95": p95("acquires"),
            "acquire_total_us_p95": p95("acquire_total_us"),
            "acquire_max_us_p95": p95("acquire_max_us"),
            "http_us_p95_on_same_pool_sample": p95("http_us"),
        })
    return {
        "schema_version": 1,
        "environment": "isolated GitHub runner, PostgreSQL 18 / tor_ci, SQLx pool max=3",
        "sampling": "Pseudo-random ~1/16 Book GETs; setup probes may be included",
        "measurement": "Successful SQLx Pool::acquire total checkout duration; not pure queue wait",
        "stages": stages,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--json", type=Path, required=True)
    args = parser.parse_args()
    if args.json.is_symlink():
        raise EvidenceError("refusing linked output")
    report = summarize(args.root)
    output = json.dumps(report, sort_keys=True, ensure_ascii=True)
    args.json.write_text(output + "\n", encoding="utf-8")
    print("TOR_PERF02_POOL_EVIDENCE " + output)


if __name__ == "__main__":
    main()
