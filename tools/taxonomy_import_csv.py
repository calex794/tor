#!/usr/bin/env python3
"""Atomic offline import of explicitly reviewed taxonomy CSV (migrator only)."""
import argparse
import csv
import os
from pathlib import Path
import re
import subprocess
import sys
import unicodedata

from taxonomy_admin import database_name

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
    args = parser.parse_args(argv)
    try:
        rows = checked_csv(args.reviewed_csv)
        sql = make_transaction(rows, args.curator)
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
