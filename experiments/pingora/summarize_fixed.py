#!/usr/bin/env python3
"""Summarize fixed-arrival profiles; fail on invalid equal-load samples."""
import csv
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def median(rows, field):
    return statistics.median(field(row) for row in rows)


def main():
    root = Path(sys.argv[1])
    groups = defaultdict(list)
    for path in root.glob("cpu*-r*-round*-*.profile.json"):
        sample = json.loads(path.read_text(encoding="utf-8"))
        client = sample["client"]
        if (
            client["errors"] or client["dropped"] or client["http_non200"]
            or client["goodput_rps"] != sample["rate"]
        ):
            raise SystemExit(f"invalid equal-load sample: {path}")
        key = (sample["vcpu"], sample["rate"], sample["scheme"])
        groups[key].append(sample)

    expected = {
        (cpu, rate, scheme)
        for cpu in (1, 2)
        for rate in (500, 1500, 3000)
        for scheme in ("nginx", "pingora")
    }
    if set(groups) != expected or any(len(rows) != 2 for rows in groups.values()):
        raise SystemExit(f"expected 12 groups with two measurements each; got {len(groups)}")

    with (root / "summary.csv").open("w", newline="", encoding="utf-8") as file:
        writer = csv.writer(file)
        writer.writerow([
            "vcpu", "rate", "scheme", "repeats", "median_cpu_seconds",
            "cpu_vs_nginx", "median_p50_ms", "median_p95_ms", "median_p99_ms",
            "median_rss_kib", "median_upstream_tcp_accepts",
            "median_new_client_tcp_connections", "median_context_switches",
            "context_switches_comparable",
        ])
        for cpu in (1, 2):
            for rate in (500, 1500, 3000):
                base = median(
                    groups[cpu, rate, "nginx"],
                    lambda r: r["processes"]["nginx"]["cpu_seconds"],
                )
                for scheme in ("nginx", "pingora"):
                    rows = groups[cpu, rate, scheme]
                    proc = lambda r: r["processes"][scheme]
                    cputime = median(rows, lambda r: proc(r)["cpu_seconds"])
                    rss = median(rows, lambda r: proc(r)["rss_kib"])
                    p50 = median(rows, lambda r: r["client"]["p50_ms"])
                    p95 = median(rows, lambda r: r["client"]["p95_ms"])
                    p99 = median(rows, lambda r: r["client"]["p99_ms"])
                    accepts = median(rows, lambda r: r["upstream_accepted_connections"])
                    dials = median(rows, lambda r: r["client"]["new_client_connections"])
                    valid = all(
                        proc(r).get("context_switches_comparable", False)
                        for r in rows
                    )
                    switches = (
                        median(rows, lambda r: proc(r)["context_switches"])
                        if valid else ""
                    )
                    writer.writerow([
                        cpu, rate, scheme, len(rows), f"{cputime:.3f}",
                        f"{cputime / base:.3f}", f"{p50:.3f}", f"{p95:.3f}",
                        f"{p99:.3f}", f"{rss:.0f}", f"{accepts:.1f}",
                        f"{dials:.1f}", switches, int(valid),
                    ])
                    print(
                        f"FIXED_SUMMARY vcpu={cpu} rate={rate} scheme={scheme} "
                        f"cpu_s={cputime:.3f} cpu_vs_nginx={cputime / base:.3f} "
                        f"p95_ms={p95:.3f} rss_kib={rss:.0f} "
                        f"upstream_new={accepts:.1f} context_valid={valid}"
                    )
    print("FIXED_SUMMARY_COMPLETE groups=12 measurements=24")


if __name__ == "__main__":
    main()
