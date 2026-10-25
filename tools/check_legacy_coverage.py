#!/usr/bin/env python3
"""Check locked NEwBOO route/job inventory against TOR plans (offline, fail-closed).

This checks SOURCE-SNAPSHOT INTEGRITY and the presence of domain ownership hints.
It does not claim that Rust implements the routes, that a feature flag is enabled,
or that a lexical mention proves behavioral compatibility.
"""
from __future__ import annotations

import argparse
import csv
import re
import sys
import tomllib
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SNAPSHOT_DIR = Path("docs/roadmap/legacy-inventory")
EXPECTED_FILES = 150
EXPECTED_DECLARATIONS = 560
EXPECTED_EXTRA = 8  # five app decorators + three mail.add_api_route declarations
EXPECTED_JOBS = 65
EXPECTED_JOB_FREEZES = 8

VALID_MOUNTS = {"mounted", "conditional", "frozen", "unconfirmed_mount"}
VALID_COMPARISONS = {"path_mentioned", "semantic_review", "frozen", "mount_review"}
FROZEN_ROUTES = ("book_crawler", "/book-imports/crawl", "scene_marketplace",
                 "scene_presets", "chapter_scene_", "shadow-availability")
FROZEN_JOBS = ("scene_marketplace", "book_crawler")
EXTRA_FILES = {"backend/app/main.py", "backend/app/web/routes/mail_admin_routes.py"}


def read_tsv(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as f:
        return list(csv.DictReader((x for x in f if x.strip() and not x.startswith("#")),
                                   delimiter="\t"))


def key(row: dict[str, str], *, source: bool) -> tuple[str, str, str, str]:
    return (
        row["path"] if source else row["legacy_source"],
        row["line"],
        row["method"],
        row["endpoint"],
    )


def audit(root: Path = ROOT, *, strict: bool = False) -> tuple[list[str], dict[str, int]]:
    problems: list[str] = []
    stats: dict[str, int] = {}
    base = root / SNAPSHOT_DIR
    try:
        with (root / "docs/roadmap/sources.lock.toml").open("rb") as f:
            locked_sha = tomllib.load(f)["reference"]["newboo_main_sha"]
    except (OSError, KeyError, tomllib.TOMLDecodeError) as exc:
        return [f"cannot read source revision lock: {exc}"], stats
    try:
        raw_files = sorted(base.glob("routes-part-*.tsv"))
        source_declarations: list[dict[str, str]] = []
        expected_start = 0
        for part in raw_files:
            first = part.read_text(encoding="utf-8").split("\n", 1)[0]
            match = re.search(r"sha=([0-9a-f]{40})\s+slice=(\d+):(\d+)\s+files=(\d+)\s+errors=(\d+)", first)
            if not match:
                problems.append(f"{part.name}: missing source sha/slice/errors header")
                continue
            sha, start, end, file_count, failed = match.groups()
            start, end, file_count, failed = map(int, (start, end, file_count, failed))
            if sha != locked_sha or start != expected_start or end - start != file_count or failed:
                problems.append(f"{part.name}: source pin or fetched slice inconsistent")
            expected_start = end
            source_declarations.extend(read_tsv(part))
        if expected_start != EXPECTED_FILES:
            problems.append(f"source files accounted {expected_start}, expected {EXPECTED_FILES}")
        if len(source_declarations) != EXPECTED_DECLARATIONS:
            problems.append(f"source decorators {len(source_declarations)}, expected {EXPECTED_DECLARATIONS}")
        covered = read_tsv(base / "http-route-coverage.tsv")
        jobs = read_tsv(base / "deferred-jobs.tsv")
        mount = (base / "mount-context.json").read_text(encoding="utf-8")
    except (OSError, UnicodeError, csv.Error) as exc:
        return problems + [f"missing/invalid inventory: {exc}"], stats
    import json
    try:
        context = json.loads(mount)
        if context["sha"] != locked_sha:
            problems.append("mount context SHA differs from source lock")
        if len(context.get("lists", {}).get("API_ROUTERS", [])) == 0:
            problems.append("mount context missing API_ROUTERS")
        if len(context.get("lists", {}).get("WEB_ROUTERS", [])) == 0:
            problems.append("mount context missing WEB_ROUTERS")
    except (ValueError, KeyError, TypeError) as exc:
        problems.append(f"invalid mount context: {exc}")
    if len(covered) != EXPECTED_DECLARATIONS + EXPECTED_EXTRA:
        problems.append(f"covered rows {len(covered)}, expected {EXPECTED_DECLARATIONS + EXPECTED_EXTRA}")
    if len(jobs) != EXPECTED_JOBS:
        problems.append(f"registered deferred names {len(jobs)}, expected {EXPECTED_JOBS}")

    src_keys = Counter(key(x, source=True) for x in source_declarations)
    target_keys = Counter(key(x, source=False) for x in covered if x["legacy_source"] not in EXTRA_FILES)
    if src_keys != target_keys:
        missing, added = src_keys - target_keys, target_keys - src_keys
        problems.append(f"decorator->mount inventory mismatch: missing={sum(missing.values())}, unexpected={sum(added.values())}")
    if any(n != 1 for n in src_keys.values()):
        problems.append("source decorator snapshot contains duplicate source/line/method/endpoint keys")
    if any(n != 1 for n in target_keys.values()):
        problems.append("coverage snapshot contains duplicate source/line/method/endpoint keys")

    extras = [x for x in covered if x["legacy_source"] in EXTRA_FILES]
    if len(extras) != EXPECTED_EXTRA:
        problems.append(f"manual mounted routes: found {len(extras)}, expected {EXPECTED_EXTRA}")
    if len([x for x in extras if x["legacy_source"] == "backend/app/main.py"]) != 5:
        problems.append("main service routes were lost from snapshot")
    if len([x for x in extras if x["legacy_source"].endswith("mail_admin_routes.py")]) != 3:
        problems.append("manually mounted admin mail endpoints were lost")

    states = Counter()
    comparisons = Counter()
    unresolved: list[str] = []
    for row in covered:
        state = row["mount_state"]
        states[state] += 1
        comparisons[row["comparison"]] += 1
        label = f'{row["method"]} {row["path"]} ({row["legacy_source"]}:{row["line"]})'
        if state not in VALID_MOUNTS or row["comparison"] not in VALID_COMPARISONS:
            problems.append(f"{label}: unexpected mount/comparison state")
        if state == "frozen" and row["comparison"] != "frozen":
            problems.append(f"{label}: frozen route must be frozen for comparison")
        if state != "frozen" and row["comparison"] == "frozen":
            problems.append(f"{label}: nonfrozen route marked frozen for comparison")
        if any(v in (row["legacy_source"] + row["path"]) for v in FROZEN_ROUTES) and state != "frozen":
            problems.append(f"{label}: frozen feature leaked into active TOR planning")
        if state in ("mounted", "conditional") and not row["plan_hint"].strip():
            problems.append(f"{label}: no planned task/contract hint")
        if state in ("mounted", "conditional") and row["lane"] == "-":
            problems.append(f"{label}: planned route without owning domain")
        if state == "unconfirmed_mount":
            unresolved.append(label)
        if "<dynamic>" in row["path"] and state != "frozen":
            problems.append(f"{label}: unresolved route path expression")
        for lane in row["lane"].split(","):
            if lane.strip() and lane.strip() not in "ABCDEFGHI-":
                problems.append(f"{label}: invalid lane {lane}")

    names = [x["job_name"] for x in jobs]
    if len(set(names)) != len(names):
        problems.append("duplicate registered job name")
    job_freezes = 0
    for row in jobs:
        is_excluded = any(p in row["job_name"] for p in FROZEN_JOBS)
        if is_excluded:
            job_freezes += 1
        if row["status"] not in ("registered", "frozen"):
            problems.append(f'{row["job_name"]}: unknown job decision')
        if is_excluded != (row["status"] == "frozen"):
            problems.append(f'{row["job_name"]}: missing/surplus freeze')
        if row["status"] == "registered" and (not row["plan_hint"] or row["lane"] == "-"):
            problems.append(f'{row["job_name"]}: missing target workstream')
    if job_freezes != EXPECTED_JOB_FREEZES:
        problems.append(f"frozen deferred jobs {job_freezes}, expected {EXPECTED_JOB_FREEZES}")

    stats.update({f"route_{k}": v for k, v in states.items()})
    stats.update({f"comparison_{k}": v for k, v in comparisons.items()})
    stats["source_declarations"] = len(source_declarations)
    stats["job_registered"] = len(jobs) - job_freezes
    stats["job_frozen"] = job_freezes
    stats["review_unresolved_mount"] = len(unresolved)

    if strict:
        if unresolved:
            problems.extend(["UNMOUNTED/UNVERIFIED " + x for x in unresolved[:15]])
        pending = [x for x in covered if x["comparison"] in ("semantic_review", "mount_review")]
        if pending:
            problems.append(f"{len(pending)} routes still require semantic mapping (lexical match cannot prove parity)")
            for row in pending[:10]:
                problems.append(f'REVIEW {row["method"]} {row["path"]}')
    return problems, stats


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--strict", action="store_true", help="fail on all unreviewed mappings")
    args = parser.parse_args()
    problems, stats = audit(strict=args.strict)
    summary = ", ".join(f"{k}={v}" for k, v in sorted(stats.items()))
    print(f"NEwBOO/TOR locked inventory: {summary}")
    if problems:
        for problem in problems[:50]:
            print("ERROR: " + problem, file=sys.stderr)
        if len(problems) > 50:
            print(f"... and {len(problems) - 50} further issues", file=sys.stderr)
        return 1
    print("Snapshot consistency PASSED. Behavioral parity NOT asserted.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
