#!/usr/bin/env python3
"""Compile explicit editor decisions into a canonical taxonomy import proposal.

Offline only. Does not approve, import, or connect to a database. Approval
requires a separately reviewed curator CSV and a read-only --audit-db check.
"""
import argparse
import csv
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

from taxonomy_import_csv import checked_csv, key
from taxonomy_review_export import REVIEW_COLUMNS, safe_spreadsheet_text

DECISIONS = {"approve", "skip"}
STATUSES = {"candidate", "not_staged", "blocked", "merged"}


def _integer(value, title, minimum=0):
    if not value.isascii() or not value.isdecimal():
        raise ValueError(f"{title} must be an integer")
    number = int(value)
    if number < minimum:
        raise ValueError(f"{title} must be at least {minimum}")
    return number


def _review_label(value, normalized_key):
    """Undo only a provable spreadsheet-formula escape from review export."""
    if key(value) == normalized_key:
        return value
    if (value.startswith("'") and
            value[1:].lstrip().startswith(("=", "+", "-", "@")) and
            key(value[1:]) == normalized_key):
        return value[1:]
    raise ValueError("review label and normalized key disagree")


def _safe_approval(value):
    if value.lstrip().startswith(("=", "+", "-", "@")):
        raise ValueError("approval may not contain a spreadsheet formula prefix")


def read_decisions(path: Path):
    """Reject omitted, reordered, incomplete or modified snapshot metadata."""
    if path.stat().st_size > 30_000_000:
        raise ValueError("review CSV exceeds 30 MB")
    with path.open(encoding="utf-8-sig", newline="") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames != list(REVIEW_COLUMNS):
            raise ValueError("expected exact public taxonomy review columns")
        records = list(reader)
    if not records or len(records) > 20_000:
        raise ValueError("review requires 1..20000 rows")
    expected_total = _integer(records[0]["snapshot_total"], "snapshot_total", 1)
    if expected_total != len(records):
        raise ValueError("incomplete review: exported snapshot row count differs")
    seen = set()
    selected = []
    for number, row in enumerate(records, start=2):
        if None in row or any(value is None for value in row.values()):
            raise ValueError(f"malformed review row {number}")
        if _integer(row["snapshot_total"], "snapshot_total", 1) != expected_total:
            raise ValueError(f"inconsistent snapshot_total on row {number}")
        kind = row["kind"]
        status = row["status"]
        if kind not in {"genre", "tag", "fandom"} or status not in STATUSES:
            raise ValueError(f"invalid kind or candidate status on row {number}")
        raw_key = row["normalized_key"]
        if not raw_key or len(raw_key) > 80:
            raise ValueError(f"invalid normalized key on row {number}")
        uses = _integer(row["public_uses"], "public_uses", 1)
        count = _integer(row["spelling_count"], "spelling_count", 1)
        try:
            spellings = json.loads(row["raw_spellings_json"])
        except json.JSONDecodeError as exc:
            raise ValueError(f"malformed spellings JSON on row {number}") from exc
        if not isinstance(spellings, list) or len(spellings) != count:
            raise ValueError(f"incomplete spelling list on row {number}")
        if not all(
            isinstance(s, dict)
            and set(s) == {"label", "uses"}
            and isinstance(s["label"], str)
            and isinstance(s["uses"], int)
            and not isinstance(s["uses"], bool)
            and s["uses"] >= 1
            for s in spellings
        ):
            raise ValueError(f"malformed public spelling on row {number}")
        if sum(s["uses"] for s in spellings) != uses:
            raise ValueError(f"modified public usage count on row {number}")
        normalized = key(spellings[0]["label"])
        if (row["normalized_key"] != safe_spreadsheet_text(normalized)
                or any(key(s["label"]) != normalized for s in spellings)):
            raise ValueError(f"modified normalized key or spellings on row {number}")
        review_label = _review_label(row["review_label"], normalized)
        token = kind, normalized
        if token in seen:
            raise ValueError(f"duplicate review group on row {number}")
        seen.add(token)
        candidate_id = row["term_id"]
        if status == "not_staged":
            if candidate_id:
                raise ValueError(f"unregistered group has a term ID on row {number}")
        else:
            _integer(candidate_id, "term_id", 1)
        decision = row["decision"]
        if decision not in DECISIONS:
            raise ValueError(f"missing explicit approve/skip on row {number}")
        label = row["approved_label"]
        aliases = row["approved_aliases"]
        if decision == "skip":
            if label or aliases:
                raise ValueError(f"skipped review has approval values on row {number}")
            continue
        if status not in {"candidate", "not_staged"}:
            raise ValueError(f"cannot approve blocked/merged term on row {number}")
        if not label:
            raise ValueError(f"approval needs an explicit label on row {number}")
        _safe_approval(label)
        values = aliases.split("|") if aliases else []
        for variant in values:
            _safe_approval(variant)
        if status == "candidate" and label != review_label:
            raise ValueError(f"candidate approval must retain its canonical label on row {number}")
        if (status == "not_staged" and key(label) != normalized
                and not any(key(a) == normalized for a in values)):
            raise ValueError(
                f"renamed public label must appear among approved aliases on row {number}"
            )
        selected.append((kind, label, values))
    if not selected:
        raise ValueError("no explicit approvals in review CSV")
    return selected, expected_total


def compile_review(review: Path, dest: Path):
    """Create an importer-format CSV only when every reviewed row is valid."""
    if review.resolve() == dest.resolve():
        raise ValueError("review input and approved CSV must be different paths")
    terms, total = read_decisions(review)
    # Reuse the actual importer preflight (bounds, alias collisions, Unicode).
    with TemporaryDirectory() as temp:
        scratch = Path(temp) / "preflight.csv"
        with scratch.open("w", encoding="utf-8", newline="") as output:
            writer = csv.writer(output)
            writer.writerow(("kind", "label", "aliases"))
            writer.writerows((kind, label, "|".join(aliases)) for kind, label, aliases in terms)
        checked_csv(scratch)
    with dest.open("x", encoding="utf-8-sig", newline="") as output:
        writer = csv.writer(output)
        writer.writerow(("kind", "label", "aliases"))
        writer.writerows((kind, label, "|".join(aliases)) for kind, label, aliases in terms)
    return len(terms), total


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("review_csv")
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)
    try:
        selected, total = compile_review(Path(args.review_csv), Path(args.output))
        print(f"OFFLINE ONLY: compiled {selected} explicit approvals from "
              f"{total} reviewed groups; no database changes")
        return 0
    except (ValueError, OSError, UnicodeError) as exc:
        print(f"taxonomy review compile: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
