#!/usr/bin/env python3
"""Export unresolved *public* taxonomy labels for explicit human review.

This file is deliberately NOT a valid input for taxonomy_import_csv.py.
Only public_catalog_facets is inspected, and nothing is ever approved.
"""
import argparse
import csv
import json
import os
from pathlib import Path
import sys

from taxonomy_admin import database_name, run_sql

REVIEW_SQL = """
BEGIN TRANSACTION READ ONLY;
WITH public_labels AS (
 SELECT kind,name,book_taxonomy_key(name) AS k,reference_count
 FROM public_catalog_facets
 WHERE reference_count>0
), grouped AS (
 SELECT kind,k,sum(reference_count)::bigint AS public_uses,
        count(*)::bigint AS spelling_count,
        min(name) AS example_label,
        jsonb_agg(
            jsonb_build_object('label',name,'uses',reference_count)
            ORDER BY reference_count DESC,name
        ) AS raw_spellings
 FROM public_labels GROUP BY kind,k
), unresolved AS (
 SELECT g.*,t.id AS term_id,t.label AS canonical_label,
        coalesce(t.status,'not_staged') AS status
 FROM grouped g
 LEFT JOIN book_taxonomy_terms t
   ON t.kind=g.kind AND t.search_key=g.k
 WHERE NOT EXISTS (
    SELECT 1 FROM book_taxonomy_terms active
    WHERE active.kind=g.kind AND active.status='active'
      AND (active.search_key=g.k OR EXISTS (
       SELECT 1 FROM book_taxonomy_aliases a
       WHERE a.kind=g.kind AND a.search_key=g.k AND a.term_id=active.id
      ))
 )
)
SELECT jsonb_build_object(
    'kind',u.kind,'normalized_key',u.k,
    'review_label',coalesce(u.canonical_label,u.example_label),
    'public_uses',u.public_uses,'spelling_count',u.spelling_count,
    'term_id',u.term_id,'status',u.status,
    'raw_spellings',u.raw_spellings,'total_unresolved',count(*) OVER ()
)::text
FROM unresolved u
ORDER BY u.public_uses DESC,u.kind,u.k
LIMIT :'limit'::integer;
COMMIT;
"""

REVIEW_COLUMNS = (
    "kind", "normalized_key", "public_uses", "spelling_count",
    "term_id", "status", "review_label", "raw_spellings_json",
    "decision", "approved_label", "approved_aliases",
)


def fetch_review(url: str, max_terms: int = 10000) -> list[dict]:
    """One consistent read-only SQL snapshot; refuse truncated reports."""
    if not 1 <= max_terms <= 20000:
        raise ValueError("max-terms must be 1..20000")
    data = run_sql(url, REVIEW_SQL, limit=str(max_terms))
    rows = [json.loads(line) for line in data.splitlines() if line.strip()]
    if not rows:
        return []
    total = rows[0].get("total_unresolved")
    if not isinstance(total, int) or total < len(rows) or total > max_terms:
        raise ValueError("review exceeds --max-terms; no incomplete export written")
    if len(rows) != total or any(
        not isinstance(row, dict)
        or row.get("total_unresolved") != total
        or row.get("kind") not in {"genre", "tag", "fandom"}
        or not isinstance(row.get("normalized_key"), str)
        or not isinstance(row.get("review_label"), str)
        or not isinstance(row.get("public_uses"), int)
        or not isinstance(row.get("spelling_count"), int)
        or not isinstance(row.get("raw_spellings"), list)
        or row.get("status") not in {"candidate", "blocked", "merged", "not_staged"}
        for row in rows
    ):
        raise ValueError("incomplete or invalid read-only taxonomy review")
    return rows


def safe_spreadsheet_text(value: str) -> str:
    """Do not execute formulas when public labels are opened in spreadsheet apps."""
    if value.lstrip().startswith(("=", "+", "-", "@")):
        return "'" + value
    return value


def export_review(rows: list[dict], dest: Path) -> None:
    """Create-only CSV: never overwrite a previous human-reviewed decision file."""
    with dest.open("x", encoding="utf-8-sig", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=REVIEW_COLUMNS)
        writer.writeheader()
        for row in rows:
            writer.writerow({
                "kind": row["kind"],
                "normalized_key": safe_spreadsheet_text(row["normalized_key"]),
                "public_uses": row["public_uses"],
                "spelling_count": row["spelling_count"],
                "term_id": row.get("term_id") or "",
                "status": row["status"],
                "review_label": safe_spreadsheet_text(row["review_label"]),
                "raw_spellings_json": json.dumps(
                    row["raw_spellings"], ensure_ascii=False, separators=(",", ":")
                ),
                # Deliberately no proposal masquerading as an approval.
                "decision": "",
                "approved_label": "",
                "approved_aliases": "",
            })


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path,
                        help="new review CSV path; refuses overwrite")
    parser.add_argument("--max-terms", type=int, default=10000)
    args = parser.parse_args(argv)
    try:
        url = os.environ.get("TOR_TAXONOMY_ADMIN_URL", "")
        database_name(url)  # migration-owner role, no password logging
        # Do not touch the output path until a complete, consistent snapshot
        # has been validated.
        rows = fetch_review(url, args.max_terms)
        export_review(rows, args.output)
        print(f"READ-ONLY PUBLIC REVIEW: {len(rows)} unresolved terms; "
              f"exported {args.output}; no editorial decisions or DB writes")
        return 0
    except (ValueError, RuntimeError, OSError, json.JSONDecodeError) as error:
        print(f"taxonomy review export: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
