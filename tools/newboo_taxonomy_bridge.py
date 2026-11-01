#!/usr/bin/env python3
"""NEwBOO full YDB taxonomy snapshot -> review-only TOR candidate CSV."""
import argparse
import csv
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import unicodedata
from taxonomy_import_csv import checked_csv

SOURCE = "newboo-ydb-book-taxonomies-full-snapshot"
KINDS = {"genre", "tag", "fandom"}
STATUSES = {"active", "merged", "blocked", "inactive"}


def tor_key(value):
    return " ".join(value.strip().lower().replace("ё", "е").split())


def semantic(value):
    text = unicodedata.normalize("NFKC", value).replace("ё", "е").casefold()
    text = re.sub(r"""["'«»“”„\x60]""", "", text)
    text = re.sub(r"[_-]+", " ", text)
    text = re.sub(r"\s+", " ", text).strip(" .,:;|/\\")
    repl = {"гг": "главный герой", "мс": "главный герой",
            "mc": "главный герой", "op": "сильный",
            "имба": "сильный", "имбовый": "сильный"}
    return " ".join(repl.get(word, word) for word in text.split())


def safe_name(value, limit):
    if not isinstance(value, str) or not 1 <= len(value) <= limit:
        return False
    return (value == value.strip() and "|" not in value
            and not any(ord(c) < 32 or ord(c) == 127 or c == "\\" for c in value))


def load(path):
    source = Path(path)
    if source.stat().st_size > 25_000_000:
        raise ValueError("snapshot exceeds 25 MB")
    data = json.loads(source.read_text(encoding="utf-8"))
    if not isinstance(data, dict) or data.get("source") != SOURCE:
        raise ValueError("requires full independent YDB export; capped admin API is insufficient")
    counts = data.get("expected_counts")
    if not isinstance(counts, dict) or set(counts) != {"taxonomies", "aliases"}:
        raise ValueError("snapshot must include independent source counts")
    for field in ("taxonomies", "aliases"):
        items = data.get(field)
        if not isinstance(items, list) or type(counts[field]) is not int:
            raise ValueError(f"invalid {field} collection/count")
        if counts[field] < 0 or counts[field] != len(items):
            raise ValueError(f"count mismatch for {field}")
    if not data["taxonomies"]:
        raise ValueError("empty YDB taxonomy export")
    return data


def prepare(data):
    errors, reviews, terms, all_ids, ownership, semantic_groups = [], [], [], {}, {}, {}
    for pos, row in enumerate(data["taxonomies"]):
        if not isinstance(row, dict):
            errors.append(f"taxonomy[{pos}]: invalid row")
            continue
        uid, kind, status = row.get("taxonomy_id"), row.get("taxonomy_type"), row.get("status")
        if (not isinstance(uid, str)
            or not re.fullmatch(r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", uid)
            or kind not in KINDS or status not in STATUSES):
            errors.append(f"taxonomy[{pos}]: invalid source identity/type/status")
            continue
        if uid in all_ids:
            errors.append(f"taxonomy[{pos}]: duplicate source ID")
            continue
        all_ids[uid] = row
        if status != "active":
            reviews.append({"reason": "inactive-or-merged", "id": uid, "status": status})
            continue
        name = row.get("title")
        if not safe_name(name, 40) or not 1 <= len(tor_key(name)) <= 80:
            errors.append(f"taxonomy[{pos}]: invalid canonical name")
            continue
        key = (kind, tor_key(name))
        if key in ownership:
            errors.append(f"taxonomy[{pos}]: canonical spelling collides with {ownership[key]}")
            continue
        ownership[key] = uid
        semantic_groups.setdefault((kind, semantic(name)), set()).add(uid)
        terms.append({"id": uid, "kind": kind, "label": name, "aliases": []})

    active = {row["id"]: row for row in terms}
    seen_alias_ids = set()
    for pos, row in enumerate(data["aliases"]):
        if not isinstance(row, dict):
            errors.append(f"alias[{pos}]: invalid row")
            continue
        uid, target, kind, name = (row.get("alias_id"), row.get("taxonomy_id"),
                                    row.get("taxonomy_type"), row.get("alias"))
        if not isinstance(uid, str) or not uid or uid in seen_alias_ids:
            errors.append(f"alias[{pos}]: duplicate/missing alias ID")
            continue
        seen_alias_ids.add(uid)
        if target not in all_ids or all_ids[target].get("taxonomy_type") != kind:
            errors.append(f"alias[{pos}]: missing or wrong-kind target")
            continue
        if target not in active:
            reviews.append({"reason": "alias-of-inactive-term", "id": uid, "target": target})
            continue
        if not safe_name(name, 80) or not 1 <= len(tor_key(name)) <= 80:
            errors.append(f"alias[{pos}]: invalid alias spelling")
            continue
        key = (kind, tor_key(name))
        prior = ownership.get(key)
        if prior and prior != target:
            errors.append(f"alias[{pos}]: spelling collides with {prior}")
            continue
        if prior == target:
            if tor_key(name) != tor_key(active[target]["label"]):
                reviews.append({"reason": "redundant-alias", "id": uid, "target": target})
            continue
        ownership[key] = target
        active[target]["aliases"].append(name)
        semantic_groups.setdefault((kind, semantic(name)), set()).add(target)

    for (kind, normalized), ids in sorted(semantic_groups.items()):
        if len(ids) > 1:
            reviews.append({"reason": "semantic-near-duplicate-no-auto-merge",
                            "kind": kind, "semantic_key": normalized,
                            "source_ids": sorted(ids)})
    terms.sort(key=lambda row: (row["kind"], tor_key(row["label"]), row["id"]))
    report = {
        "source": SOURCE,
        "stats": {"source_terms": len(data["taxonomies"]), "source_aliases": len(data["aliases"]),
                  "candidate_active_terms": len(terms),
                  "candidate_aliases": sum(len(row["aliases"]) for row in terms)},
        "fatal_errors": errors,
        "manual_review": reviews,
        "warning": "candidate only; does not grant publication approval",
    }
    return terms, report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot")
    parser.add_argument("--out", required=True)
    parser.add_argument("--report", required=True)
    args = parser.parse_args(argv)
    try:
        source, output, report_path = (Path(args.snapshot).resolve(),
                                       Path(args.out).resolve(),
                                       Path(args.report).resolve())
        if len({source, output, report_path}) != 3:
            raise ValueError("source, candidate and report must be distinct paths")
        data = load(source)
        terms, report = prepare(data)
        if len(terms) > 10_000:
            report["fatal_errors"].append("source exceeds TOR importer 10,000-term limit")
        report_path.write_text(
            json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        if report["fatal_errors"]:
            raise ValueError(f"{len(report['fatal_errors'])} conflicts; inspect report")
        # Write and revalidate a temporary CSV; never leave a partially
        # accepted candidate file that a curator could accidentally import.
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="w", encoding="utf-8", newline="", suffix=".csv",
                prefix=".newboo-candidates-", dir=output.parent, delete=False,
            ) as handle:
                temporary = Path(handle.name)
                writer = csv.writer(handle)
                writer.writerow(("kind", "label", "aliases"))
                for term in terms:
                    writer.writerow((term["kind"], term["label"], "|".join(term["aliases"])))
            checked_csv(str(temporary))
            os.replace(temporary, output)
            temporary = None
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
        print(f"{len(terms)} candidate terms; {len(report['manual_review'])} review notes")
        return 0
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as exc:
        print(f"NEwBOO bridge: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
