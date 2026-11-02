#!/usr/bin/env python3
"""Atomic offline import of explicitly reviewed taxonomy CSV (migrator only)."""
import argparse
import csv
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import unicodedata

from taxonomy_admin import database_name, run_sql

def key(value):
    # Mirrors SQL NFKC, whitespace and ё/е; PostgreSQL locale is authoritative.
    normalized = unicodedata.normalize("NFKC", value)
    return " ".join(normalized.lower().replace("ё", "е").split())

def checked_csv(file_path):
    source = Path(file_path)
    if source.stat().st_size > 5_000_000:
        raise ValueError("reviewed CSV exceeds 5 MB")
    terms, seen = [], {}
    with source.open(encoding="utf-8-sig", newline="") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames != ["kind", "label", "aliases"]:
            raise ValueError("expected exact columns kind,label,aliases")
        for lineno, row in enumerate(reader, start=2):
            if len(terms) >= 10_000:
                raise ValueError("maximum 10,000 approved terms per batch")
            if None in row or any(value is None for value in row.values()):
                raise ValueError(f"invalid CSV row {lineno}")
            kind, label, raw_aliases = row["kind"], row["label"], row["aliases"]
            if kind not in {"genre", "tag", "fandom"}:
                raise ValueError(f"invalid taxonomy type on row {lineno}")
            aliases = raw_aliases.split("|") if raw_aliases else []
            if len(aliases) > 10:
                raise ValueError(f"too many aliases on row {lineno}")
            for name, maxlen in [(label, 40)] + [(a, 80) for a in aliases]:
                if not 1 <= len(name) <= maxlen or name != name.strip():
                    raise ValueError(f"invalid label length or whitespace on row {lineno}")
                if any(ord(c) < 32 or ord(c) == 127 or c == "\\" for c in name):
                    raise ValueError(f"unsafe control/backslash on row {lineno}")
                normalized_key = key(name)
                if not 1 <= len(normalized_key) <= 80 or any(
                    ord(c) < 32 or ord(c) == 127 for c in normalized_key
                ):
                    raise ValueError(f"invalid normalized key on row {lineno}")
                token = (kind, normalized_key)
                if token in seen:
                    raise ValueError(f"colliding labels/aliases at rows {seen[token]} and {lineno}")
                seen[token] = lineno
            terms.append((kind, label, aliases))
    if not terms:
        raise ValueError("empty reviewed taxonomy")
    return terms


# Single read-only database audit of *proposed* labels/aliases. All matching is
# performed with PostgreSQL book_taxonomy_key, not Python's locale approximation.
# The public reference count reads only PERF-233's public projection.
AUDIT_SQL = """
BEGIN TRANSACTION READ ONLY;
WITH submitted AS (
 SELECT (r.value->>'ordinal')::int AS ordinal,
        r.value->>'kind' AS kind,
        r.value->>'label' AS label,
        r.value->'aliases' AS aliases,
        book_taxonomy_key(r.value->>'label') AS k
 FROM jsonb_array_elements(:'records'::jsonb) AS r(value)
), matched AS (
 SELECT s.*,t.id AS term_id,t.label AS db_label,t.status AS db_status,
        a.term_id AS canonical_alias_owner
 FROM submitted s
 LEFT JOIN book_taxonomy_terms t
   ON t.kind=s.kind AND t.search_key=s.k
 LEFT JOIN book_taxonomy_aliases a
   ON a.kind=s.kind AND a.search_key=s.k
), classified AS (
 SELECT m.*,
   CASE
     WHEN m.canonical_alias_owner IS NOT NULL THEN 'canonical_is_alias'
     WHEN m.term_id IS NULL THEN 'create'
     WHEN m.db_status='active' AND m.db_label=m.label THEN 'unchanged'
     WHEN m.db_status='candidate' AND m.db_label=m.label
       THEN 'approve_candidate'
     ELSE 'canonical_conflict'
   END AS action
 FROM matched m
)
SELECT jsonb_build_object(
 'row', c.ordinal, 'kind', c.kind, 'label', c.label, 'action', c.action,
 'term_id', c.term_id, 'db_label', c.db_label,
 'db_status', coalesce(c.db_status,'not_staged'),
 'canonical_alias_owner', c.canonical_alias_owner,
 'public_uses', pub.uses, 'public_spellings', pub.spellings,
 'alias_conflicts', aliases.conflicts,
 'can_apply', c.action IN ('create','approve_candidate','unchanged')
              AND aliases.conflicts='[]'::jsonb
)::text
FROM classified c
LEFT JOIN LATERAL (
 SELECT coalesce(sum(f.reference_count),0)::bigint AS uses,
        count(*)::bigint AS spellings
 FROM public_catalog_facets f
 WHERE f.kind=c.kind AND f.reference_count>0
   AND book_taxonomy_key(f.name)=c.k
) pub ON true
LEFT JOIN LATERAL (
 SELECT coalesce(jsonb_agg(
   jsonb_build_object('alias',v.name,'canonical_id',t.id,
                      'existing_alias_owner',a.term_id)
   ORDER BY v.ordinal
 ) FILTER (WHERE t.id IS NOT NULL OR
          (a.term_id IS NOT NULL AND a.term_id IS DISTINCT FROM c.term_id)),
 '[]'::jsonb) AS conflicts
 FROM jsonb_array_elements_text(c.aliases)
   WITH ORDINALITY AS v(name,ordinal)
 LEFT JOIN book_taxonomy_terms t
   ON t.kind=c.kind AND t.search_key=book_taxonomy_key(v.name)
 LEFT JOIN book_taxonomy_aliases a
   ON a.kind=c.kind AND a.search_key=book_taxonomy_key(v.name)
) aliases ON true
ORDER BY c.ordinal;
COMMIT;
"""

def audit_terms(terms, url):
    """Return structured preview; never approve or mutate any term."""
    report = []
    # Bounded psql argv even for a 10,000-term CSV; each SQL transaction
    # is explicitly READ ONLY and uses psql variable literal quoting.
    for start in range(0, len(terms), 50):
        batch = [
            {"ordinal": start + i + 1, "kind": kind, "label": label,
             "aliases": aliases}
            for i, (kind, label, aliases) in enumerate(terms[start:start + 50])
        ]
        output = run_sql(
            url, AUDIT_SQL,
            records=json.dumps(batch, ensure_ascii=False, separators=(",", ":")),
        )
        for line in output.splitlines():
            result = json.loads(line)
            if not isinstance(result, dict):
                raise RuntimeError("invalid taxonomy audit record")
            report.append(result)
    if [record.get("row") for record in report] != list(range(1, len(terms) + 1)):
        raise RuntimeError("taxonomy audit returned incomplete/out-of-order results")
    return report

def sql_literal(value):
    if any(ord(c) < 32 or c == "\\" for c in value):
        raise ValueError("unsafe SQL literal")
    return "'" + value.replace("'", "''") + "'"

def make_transaction(terms, curator):
    if not re.fullmatch(r"[a-zA-Z0-9_.-]{3,80}", curator):
        raise ValueError("invalid audit curator label")
    actor = sql_literal(curator)
    statements = [
        "BEGIN;",
        "SET LOCAL statement_timeout = '150s';",
        "SET LOCAL standard_conforming_strings = on;",
    ]
    for kind, label, aliases in terms:
        k, t = sql_literal(kind), sql_literal(label)
        if not aliases:
            statements.append(f"SELECT admin_taxonomy_add_term({k},{t},{actor});")
            continue
        alias_values = ",".join(f"({sql_literal(a)})" for a in aliases)
        statements.append(
            f"WITH created AS (SELECT admin_taxonomy_add_term({k},{t},{actor}) AS id) "
            f"SELECT admin_taxonomy_add_alias({k},created.id,aliases.label,{actor}) "
            f"FROM created CROSS JOIN (VALUES {alias_values}) AS aliases(label);"
        )
    return "\n".join(statements + ["COMMIT;", ""])

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reviewed_csv")
    parser.add_argument("--curator", required=True)
    parser.add_argument("--confirm-db")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--audit-db", action="store_true", help="read-only database conflict report as JSON Lines")
    args = parser.parse_args(argv)
    try:
        rows = checked_csv(args.reviewed_csv)
        sql = make_transaction(rows, args.curator)
        if args.audit_db:
            if args.apply or args.confirm_db:
                raise ValueError("--audit-db is read-only and cannot be combined with --apply/--confirm-db")
            url = os.environ.get("TOR_TAXONOMY_ADMIN_URL", "")
            database_name(url)  # require the same explicitly selected curator role
            audit = audit_terms(rows, url)
            for record in audit:
                print(json.dumps(record, ensure_ascii=False, sort_keys=True))
            conflicts = sum(not item["can_apply"] for item in audit)
            print(
                f"READ-ONLY AUDIT: {len(audit)} terms, {conflicts} conflicts; "
                "no database changes or editorial approvals",
                file=sys.stderr,
            )
            return 0 if conflicts == 0 else 2
        if not args.apply:
            print(f"DRY RUN: {len(rows)} reviewed terms, no database changes")
            return 0
        url = os.environ.get("TOR_TAXONOMY_ADMIN_URL", "")
        dbname = database_name(url)
        if args.confirm_db != dbname:
            raise ValueError(f"apply requires --confirm-db {dbname}")
        # One transaction, one connection; any error rolls everything back.
        result = subprocess.run(
            ["psql", "-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "-d", url],
            input=sql, text=True, capture_output=True, timeout=180, check=False,
        )
        if result.returncode:
            raise RuntimeError(result.stderr.strip()[-1200:] or "psql batch failed")
        print(f"Processed {len(rows)} approved terms atomically (exact repeats unchanged)")
        return 0
    except (ValueError, OSError, RuntimeError, subprocess.TimeoutExpired) as exc:
        print(f"taxonomy CSV importer: {exc}", file=sys.stderr)
        return 2

if __name__ == "__main__":
    raise SystemExit(main())
