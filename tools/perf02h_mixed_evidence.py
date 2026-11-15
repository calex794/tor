#!/usr/bin/env python3
"""PERF-02h: fail-closed, numeric-only mixed business-load summary.

Requires actual k6 JSON. No credentials, SQL, HTTP bodies, account IDs, or URLs.
Optional capacity stages can fail but cannot masquerade as a passing result.
"""
import argparse
import csv
import json
import math
from pathlib import Path

RATES = (25, 50, 100)
DURATIONS = {25: 60, 50: 15, 100: 15}
COMPOSITION = {25: (13, 10, 2), 50: (26, 20, 4), 100: (52, 40, 8)}
MEASURES = {
    "public_book_get": "perf252_public_book_ms",
    "free_progress_write": "perf252_progress_save_ms",
    "paid_purchase": "perf252_purchase_ms",
}
MAX_INPUT_BYTES = 8 * 1024 * 1024


class EvidenceError(ValueError):
    pass


def read_text(path):
    if not path.is_file() or path.is_symlink() or path.stat().st_size > MAX_INPUT_BYTES:
        raise EvidenceError("missing, linked or oversized mixed-load evidence")
    return path.read_text(encoding="utf-8")


def stage_results(root):
    rows = list(csv.reader(read_text(root / "results.tsv").splitlines(), delimiter="\t"))
    if not rows or rows[0] != ["journeys_per_second", "outcome"]:
        raise EvidenceError("invalid PERF-02h result header")
    if len(rows) != len(RATES) + 1:
        raise EvidenceError("incomplete PERF-02h rate stages")
    result = {}
    for row in rows[1:]:
        if len(row) != 2 or not row[0].isdigit() or row[1] not in ("PASS", "FAIL"):
            raise EvidenceError("malformed mixed-load stage")
        rate = int(row[0])
        if rate in result or rate not in RATES:
            raise EvidenceError("duplicate or unexpected mixed-load stage")
        result[rate] = row[1]
    if set(result) != set(RATES) or result[25] != "PASS":
        raise EvidenceError("required 25-journey baseline was not successful")
    return result


def number(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise EvidenceError("invalid latency number")
    if not math.isfinite(value) or value < 0:
        raise EvidenceError("non-finite or negative latency")
    return round(float(value), 3)


def latency(root, rate):
    summary = json.loads(read_text(root / f"summary-{rate}.json"))
    metrics = summary["metrics"]
    values_by_name = {}
    for output_name, metric_name in MEASURES.items():
        metric = metrics[metric_name]
        values = metric.get("values", metric)
        row = {
            "p50_ms": number(values["med"]),
            "p95_ms": number(values["p(95)"]),
            "p99_ms": number(values["p(99)"]),
        }
        if not row["p50_ms"] <= row["p95_ms"] <= row["p99_ms"]:
            raise EvidenceError("latency percentiles out of order")
        values_by_name[output_name] = row
    return values_by_name


def summarize(root):
    results = stage_results(root)
    stages = []
    for rate in RATES:
        outcome = results[rate]
        try:
            metrics = latency(root, rate)
        except (EvidenceError, KeyError, TypeError, json.JSONDecodeError) as exc:
            if outcome == "PASS":
                raise EvidenceError(f"missing required k6 metrics for {rate}/s") from exc
            metrics = None  # failure does not mean a measured zero
        writers, readers, buyers = COMPOSITION[rate]
        stages.append({
            "target_journeys_per_second": rate,
            "duration_seconds": DURATIONS[rate],
            "writer_journeys_per_second": writers,
            "public_journeys_per_second": readers,
            "paid_journeys_per_second": buyers,
            "k6_outcome": outcome,
            "http_latency": metrics,
        })
    return {
        "schema_version": 1,
        "environment": "disposable PostgreSQL 18 / tor_ci, same tor-web release process, pool max=3",
        "scope": "journeys per second; not individual HTTP requests per second",
        "paid_chapters": "disjoint IDs across sequential runs, 300 total available",
        "financial_gate": "verified independently against PG purchase facts, wallet entries, owned rights and remaining balances",
        "stages": stages,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--json", required=True, type=Path)
    args = parser.parse_args()
    if args.json.is_symlink():
        raise EvidenceError("linked JSON output not allowed")
    result = summarize(args.root)
    output = json.dumps(result, sort_keys=True, ensure_ascii=True)
    args.json.write_text(output + "\n", encoding="utf-8")
    print("PERF02H_MIXED_LOAD " + output)


if __name__ == "__main__":
    main()
