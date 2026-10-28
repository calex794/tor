#!/usr/bin/env python3
"""Summarize repeated HTTP proxy measurements; use stdlib only."""
import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path

if len(sys.argv) != 3:
    raise SystemExit("usage: summarize.py measurements.txt summary.csv")

groups = defaultdict(list)
for line in Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
    if "TOR_PINGORA_METRIC " not in line or "round=" not in line:
        continue
    values = dict(token.split("=", 1) for token in line.split() if "=" in token)
    for key in ("route", "scheme", "round", "rps", "p50_us", "p95_us", "p99_us"):
        if key not in values:
            raise SystemExit(f"incomplete metric: {line}")
    errors = sum(
        int(values[key])
        for key in (
            "connect_errors", "read_errors", "write_errors",
            "timeout_errors", "status_errors",
        )
    )
    if errors:
        raise SystemExit(f"request errors found: {line}")
    groups[(values["route"], values["scheme"])].append(values)

routes = ("catalog", "books/1", "api/v1/books")
schemes = ("nginx", "pingora", "nginx_pingora")
if len(groups) != len(routes) * len(schemes):
    raise SystemExit(f"expected 9 route/scheme groups, got {len(groups)}")
if any(set(int(row["round"]) for row in groups[route, scheme]) != set(range(1, 6))
       for route in routes for scheme in schemes):
    raise SystemExit("expected five independent rounds for every route/scheme")

median_rps = {
    (route, scheme): statistics.median(float(row["rps"]) for row in groups[route, scheme])
    for route in routes for scheme in schemes
}
with open(sys.argv[2], "w", encoding="utf-8", newline="") as output:
    writer = csv.writer(output)
    writer.writerow(
        ["route", "scheme", "repeats", "median_rps", "rps_vs_nginx",
         "median_p50_ms", "median_p95_ms", "median_p99_ms"]
    )
    for route in routes:
        for scheme in schemes:
            sample = groups[(route, scheme)]
            delays = [
                statistics.median(float(row[key]) for row in sample) / 1000
                for key in ("p50_us", "p95_us", "p99_us")
            ]
            throughput = median_rps[(route, scheme)]
            relative = throughput / median_rps[(route, "nginx")]
            writer.writerow([
                route, scheme, len(sample), f"{throughput:.2f}",
                f"{relative:.3f}", *(f"{value:.3f}" for value in delays)
            ])
            print(
                f"BENCHMARK_SUMMARY route={route} scheme={scheme} "
                f"median_rps={throughput:.2f} vs_nginx={relative:.3f} "
                f"median_p50_ms={delays[0]:.3f} median_p95_ms={delays[1]:.3f} "
                f"median_p99_ms={delays[2]:.3f} repeats={len(sample)} "
                f"build=release"
            )
print("BENCHMARK_SUMMARY_COMPLETE groups=9 measurements=45")
