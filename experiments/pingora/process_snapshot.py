#!/usr/bin/env python3
"""Read process CPU/RSS and per-thread context switches from host /proc.

The experiment runs on a disposable runner. /proc/<pid>/status only reports
context switches of the leader thread; aggregate /proc/<pid>/task instead.
"""
import json
import os
from pathlib import Path
import subprocess
import sys


def status_fields(path):
    fields = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if ":" in line:
            key, value = line.split(":", 1)
            tokens = value.strip().split()
            if tokens:
                fields[key] = tokens[0]
    return fields


def read_process(pid):
    root = Path(f"/proc/{pid}")
    fields = (root / "stat").read_text().rsplit(") ", 1)[1].split()
    cpu_ticks = int(fields[11]) + int(fields[12])
    rss_kib = int(status_fields(root / "status").get("VmRSS", 0))
    thread_ids = []
    voluntary = 0
    involuntary = 0
    for entry in sorted((root / "task").iterdir(), key=lambda item: item.name):
        if not entry.name.isdigit():
            continue
        try:
            status = status_fields(entry / "status")
        except FileNotFoundError:
            continue
        thread_ids.append(int(entry.name))
        voluntary += int(status["voluntary_ctxt_switches"])
        involuntary += int(status["nonvoluntary_ctxt_switches"])
    if not thread_ids:
        raise RuntimeError(f"no readable threads for PID {pid}")
    return cpu_ticks, rss_kib, voluntary, involuntary, thread_ids


def main():
    raw = subprocess.check_output(
        ["docker", "top", sys.argv[1], "-eo", "pid,comm"], text=True
    )
    groups = {"nginx": [], "pingora": [], "axum": []}
    for line in raw.splitlines()[1:]:
        row = line.split()
        if len(row) != 2 or not row[0].isdigit():
            continue
        pid, command = int(row[0]), row[1]
        component = (
            "nginx" if command.startswith("nginx") else
            "pingora" if command.startswith("tor-pingora") else
            "axum" if command.startswith("axum_fixture") else None
        )
        if component:
            groups[component].append(pid)

    result = {"ticks_per_second": os.sysconf("SC_CLK_TCK"), "components": {}}
    for component, pids in groups.items():
        if not pids:
            raise RuntimeError(f"missing {component} process")
        total = {
            "cpu_ticks": 0, "rss_kib": 0,
            "voluntary_switches": 0, "involuntary_switches": 0,
            "pids": sorted(pids), "thread_ids": [],
        }
        for pid in pids:
            cpu, rss, voluntary, involuntary, threads = read_process(pid)
            total["cpu_ticks"] += cpu
            total["rss_kib"] += rss
            total["voluntary_switches"] += voluntary
            total["involuntary_switches"] += involuntary
            total["thread_ids"].extend(threads)
        total["thread_ids"].sort()
        result["components"][component] = total
    print(json.dumps(result))


if __name__ == "__main__":
    main()
