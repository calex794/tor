#!/usr/bin/env python3
"""Compare before/after process counters for one fixed-rate test."""
import json
import sys


def load(path):
    with open(path, encoding="utf-8") as file:
        return json.load(file)


def main():
    prefix, before_count, after_count, cpus, rate, round_no, scheme = sys.argv[1:]
    before = load(prefix + ".before.json")
    after = load(prefix + ".after.json")
    client = load(prefix + ".client.json")
    if before["ticks_per_second"] != after["ticks_per_second"]:
        raise SystemExit("clock rate changed")
    delta = {}
    for name in ("nginx", "pingora", "axum"):
        a, b = before["components"][name], after["components"][name]
        if set(a["pids"]) != set(b["pids"]):
            raise SystemExit("process PID changed: " + name)
        comparable = bool(a.get("thread_ids")) and a.get("thread_ids") == b.get("thread_ids")
        switches = (
            b["voluntary_switches"] + b["involuntary_switches"]
            - a["voluntary_switches"] - a["involuntary_switches"]
        ) if comparable else None
        if switches is not None and switches < 0:
            comparable, switches = False, None
        delta[name] = {
            "cpu_seconds": round(
                (b["cpu_ticks"] - a["cpu_ticks"]) / before["ticks_per_second"], 3
            ),
            "rss_kib": b["rss_kib"],
            "thread_count": len(b.get("thread_ids", [])),
            "context_switches": switches,
            "context_switches_comparable": comparable,
        }
    upstream_new = int(after_count) - int(before_count) - 1
    if upstream_new < 0:
        raise SystemExit("negative upstream connections")
    result = {
        "vcpu": int(cpus), "rate": int(rate), "round": int(round_no),
        "scheme": scheme, "client": client, "processes": delta,
        "upstream_accepted_connections": upstream_new,
    }
    with open(prefix + ".profile.json", "w", encoding="utf-8") as file:
        json.dump(result, file, indent=2)
    print("PROFILE_RESULT", json.dumps({
        "vcpu": cpus, "rate": rate, "scheme": scheme,
        "round": round_no, "goodput": client["goodput_rps"],
        "p95": client["p95_ms"],
        "proxy_cpu": delta[scheme]["cpu_seconds"],
        "rss_kib": delta[scheme]["rss_kib"],
        "context_switches": delta[scheme]["context_switches"],
        "thread_count": delta[scheme]["thread_count"],
        "upstream_new": upstream_new,
        "errors": client["errors"], "dropped": client["dropped"],
    }))


if __name__ == "__main__":
    main()
