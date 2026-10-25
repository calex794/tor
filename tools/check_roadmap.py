#!/usr/bin/env python3
"""Fail closed if TOR roadmap loses an accepted scenario or an executable task."""
from __future__ import annotations

import csv
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXCLUDED = {"S040": "не переносить", "S053": "заморожено", "S054": "заморожено",
            "S055": "заморожено", "S056": "заморожено", "S067": "не переносить",
            "S069": "не переносить"}
LANE_TASK_COUNTS = {"A": 11, "B": 16, "C": 12, "D": 13, "E": 14,
                    "F": 14, "G": 12, "H": 12, "I": 13}
TASK_RE = re.compile(r"^([A-I])-(\d{2})$")
S_RE = re.compile(r"^S\d{3}$")
INVALID = re.compile(r"\b(TODO|TBD|TBC|FIXME)\b|проработать\s+потом|решить\s+позже", re.I)


def load(path: Path):
    with path.open(encoding="utf-8", newline="") as f:
        meaningful = (line for line in f if line.strip() and not line.startswith("#"))
        rows = list(csv.DictReader(meaningful, delimiter="\t"))
    return rows


def check(root: Path = ROOT) -> list[str]:
    errors: list[str] = []
    base = root / "docs" / "roadmap"
    scenario_file, task_file = base / "scenarios.tsv", base / "tasks.tsv"
    if not scenario_file.exists() or not task_file.exists():
        return [f"roadmap missing: {scenario_file} or {task_file}"]
    try:
        scenarios, tasks = load(scenario_file), load(task_file)
    except (OSError, UnicodeError, csv.Error) as exc:
        return [f"invalid roadmap TSV: {exc}"]

    expected_s = [f"S{n:03d}" for n in range(1, 70)]
    codes = [s.get("code", "") for s in scenarios]
    if codes != expected_s:
        errors.append("scenarios.tsv must contain S001..S069 exactly once and in order")
    owners: dict[str, str] = {}
    for s in scenarios:
        code, owner = s.get("code", ""), s.get("owner", "")
        if code in EXCLUDED:
            if owner != "-" or s.get("decision") != EXCLUDED[code]:
                errors.append(f"{code}: excluded/frozen status or owner changed")
        elif owner not in LANE_TASK_COUNTS or owner == "H":
            errors.append(f"{code}: missing/invalid owning lane {owner}")
        else:
            owners[code] = owner
        if not s.get("title") or not s.get("priority") or not s.get("acceptance"):
            errors.append(f"{code}: missing title/priority/acceptance")
        if not s.get("notion_scenario_url", "").startswith("https://app.notion.com/"):
            errors.append(f"{code}: missing canonical source scenario link")
        if not s.get("source_code_url", "").startswith("https://github.com/"):
            errors.append(f"{code}: missing source-code link")
        if not s.get("target_contract"):
            errors.append(f"{code}: missing chosen TOR entrypoint")
        if INVALID.search(s.get("target_contract", "") + " " + s.get("acceptance", "")):
            errors.append(f"{code}: unresolved contract/acceptance placeholder")
    if len(owners) != 62:
        errors.append(f"expected 62 assigned scenarios, got {len(owners)}")

    counts: Counter[str] = Counter()
    keys: set[str] = set()
    task_refs: dict[str, list[str]] = defaultdict(list)
    for t in tasks:
        task_id, lane = t.get("id", ""), t.get("lane", "")
        m = TASK_RE.fullmatch(task_id)
        if not m or lane != m.group(1):
            errors.append(f"invalid task lane/id: {task_id}/{lane}")
            continue
        if task_id in keys:
            errors.append(f"{task_id}: duplicated task")
        keys.add(task_id)
        counts[lane] += 1
        for field in ("day", "related_s_ids", "target_crate", "implementation",
                      "code_data", "acceptance", "test_entry"):
            if not t.get(field, "").strip():
                errors.append(f"{task_id}: {field} missing")
        if INVALID.search(" ".join(t.values())):
            errors.append(f"{task_id}: unresolved TODO / placeholder")
        if t.get("target_crate") != "tor-" + ("web" if lane in {"H", "I"} else
               t.get("target_crate", "").removeprefix("tor-")):
            errors.append(f"{task_id}: bad target crate")
        test = t.get("test_entry", "")
        if lane != "I" and f" {lane.lower()}_{m.group(2)}_" not in test:
            errors.append(f"{task_id}: missing deterministic test selector")
        if "cargo test" not in test and "GitHub CI" not in test:
            errors.append(f"{task_id}: no runnable test entry")
        for code in t.get("related_s_ids", "").split(","):
            if not S_RE.fullmatch(code):
                errors.append(f"{task_id}: invalid S reference {code}")
            elif code not in codes:
                errors.append(f"{task_id}: unknown S reference {code}")
            elif owners.get(code) == lane:
                task_refs[code].append(task_id)
    for lane, expected in LANE_TASK_COUNTS.items():
        if counts[lane] != expected:
            errors.append(f"lane {lane}: expected {expected} tasks, got {counts[lane]}")
    if len(tasks) != 117:
        errors.append(f"expected 117 tasks, got {len(tasks)}")
    for code, lane in owners.items():
        if not task_refs[code]:
            errors.append(f"{code}: owning lane {lane} has no executable task")
    if owners.get("S032") != "D" or "D-05" not in task_refs.get("S032", []):
        errors.append("S032 book layer ownership must be commerce D/D-05")
    return errors


if __name__ == "__main__":
    issues = check()
    for issue in issues:
        print("FAIL:", issue, file=sys.stderr)
    if issues:
        sys.exit(1)
    print("TOR roadmap coverage: 69/69 classified, 62 owned, 7 excluded, 117 tasks, 9 lanes: PASS")
