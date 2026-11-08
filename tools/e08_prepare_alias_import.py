#!/usr/bin/env python3
"""Prepare a reviewed NEwBOO -> TOR mail Book alias import; never connect to a DB.

Only explicitly verified Book identities can yield SQL. Legacy numeric IDs
are deliberately excluded because TOR's numeric book IDs have priority.
"""

import argparse
import csv
import re
import sys
from collections import defaultdict
from pathlib import Path
from uuid import UUID

ALIAS = re.compile(r"[a-z][a-z0-9_.-]{0,127}\Z")
POSITIVE_ID = re.compile(r"[1-9][0-9]*\Z")
SOURCE_FIELDS = ("book_id", "public_id", "slug")
MAP_FIELDS = (
    "newboo_book_id", "tor_book_id", "tor_source_identity",
    "tor_target_language", "reviewed",
)


class ImportErrorDetail(ValueError):
    """A non-recoverable identity or input ambiguity."""


def read_csv(path, required):
    with path.open("r", encoding="utf-8-sig", newline="") as stream:
        reader = csv.DictReader(stream)
        header = reader.fieldnames or []
        if len(header) != len(set(header)) or any(field not in header for field in required):
            raise ImportErrorDetail(f"{path}: required unique columns: {', '.join(required)}")
        for line_number, row in enumerate(reader, 2):
            if None in row or any(value is None for value in row.values()):
                raise ImportErrorDetail(f"{path}:{line_number}: malformed CSV record")
            yield line_number, row


def positive_id(value, label):
    value = value.strip()
    if not POSITIVE_ID.fullmatch(value) or int(value) > 9223372036854775807:
        raise ImportErrorDetail(f"{label}: expected canonical positive int64")
    return int(value)


def safe_alias(raw):
    # The Rust mail parser only accepts literal ASCII tokens. Lowercasing
    # Unicode first would incorrectly turn U+212A (Kelvin sign) into "k".
    if raw != raw.strip() or not raw.isascii():
        return None
    raw = raw.lower()
    # The mail URL parser strips trailing dots as sentence punctuation.
    return raw if ALIAS.fullmatch(raw) and not raw.endswith(".") else None


def identity(value, label, maximum):
    if not value or value != value.strip() or len(value) > maximum or any(ord(ch) < 32 or ord(ch) == 127 for ch in value):
        raise ImportErrorDetail(f"{label}: invalid identity field")
    return value


def source_book_key(raw, label):
    """Mirror NEwBOO's numeric/UUID/public BookReference identities.

    TOR's target book_id stays a positive int64. The source Book identity
    is independent and can be a string or a UUID; never guess a target ID.
    """
    value = identity(raw, label, 256)
    numeric = value.lstrip("+-")
    if numeric.isdigit():
        if not numeric.isascii() or not re.fullmatch(r"[+-]?[0-9]+", value):
            raise ImportErrorDetail(f"{label}: unsupported numeric spelling")
        number = int(value)
        if not 1 <= number <= 9223372036854775807:
            raise ImportErrorDetail(f"{label}: source numeric Book ID is out of range")
        return ("numeric", str(number))
    try:
        return ("uuid", str(UUID(value)))
    except ValueError:
        return ("public", value)


def prepare(source_path, mapping_path):
    sources = {}
    owners = defaultdict(set)
    for line, row in read_csv(source_path, SOURCE_FIELDS):
        source_id = source_book_key(row["book_id"], f"{source_path}:{line}:book_id")
        if source_id in sources:
            raise ImportErrorDetail(f"{source_path}:{line}: duplicate book_id {source_id}")
        sources[source_id] = row
        for field in SOURCE_FIELDS:
            alias = safe_alias(row[field])
            if alias:
                owners[alias].add(source_id)

    if not sources:
        raise ImportErrorDetail("source export has no books")
    aliases = {}
    seen_sources = set()
    seen_targets = set()
    unsupported = []
    for line, row in read_csv(mapping_path, MAP_FIELDS):
        label = f"{mapping_path}:{line}"
        source_id = source_book_key(row["newboo_book_id"], f"{label}:newboo_book_id")
        target_id = positive_id(row["tor_book_id"], f"{label}:tor_book_id")
        if row["reviewed"].strip().lower() != "yes":
            raise ImportErrorDetail(f"{label}: mapping must be explicitly reviewed=yes")
        if source_id not in sources:
            raise ImportErrorDetail(f"{label}: source book {source_id} absent from export")
        if source_id in seen_sources or target_id in seen_targets:
            raise ImportErrorDetail(f"{label}: repeated source or TOR Book mapping")
        seen_sources.add(source_id)
        seen_targets.add(target_id)
        source_identity = identity(row["tor_source_identity"], f"{label}:tor_source_identity", 256)
        language = identity(row["tor_target_language"], f"{label}:tor_target_language", 32)
        book_aliases = set()
        for field in SOURCE_FIELDS:
            raw = sources[source_id][field]
            if not raw:
                continue
            alias = safe_alias(raw)
            if not alias:
                # A numeric/UUID source book_id is still a valid identity,
                # even if its spelling is not a supported mail URL alias.
                if field != "book_id":
                    unsupported.append((sources[source_id]["book_id"], field, raw))
                continue
            if len(owners[alias]) != 1:
                raise ImportErrorDetail(f"{label}: alias {alias!r} belongs to multiple source books")
            book_aliases.add(alias)
        if not book_aliases:
            raise ImportErrorDetail(f"{label}: book {sources[source_id]['book_id']} has no safe nonnumeric alias")
        for alias in book_aliases:
            if alias in aliases:
                raise ImportErrorDetail(f"{label}: repeated alias {alias!r}")
            aliases[alias] = (target_id, source_identity, language)

    if not aliases:
        raise ImportErrorDetail("no reviewed aliases to import")
    return sorted(aliases.items()), unsupported


def sql_literal(text):
    return "'" + text.replace("'", "''") + "'"


def render_sql(entries):
    values = ",\n".join(
        "    (" + ",".join((sql_literal(alias), str(book_id), sql_literal(source_identity), sql_literal(language))) + ")"
        for alias, (book_id, source_identity, language) in entries
    )
    return """-- E-08/S057 reviewed mail Book aliases. Produced offline; NO DB accessed.
-- Execute only after human review and a disposable PostgreSQL dry-run.
-- Numeric legacy references and unverified identities are intentionally absent.
BEGIN;
SET LOCAL standard_conforming_strings = on;
LOCK TABLE book_legacy_mail_refs IN SHARE ROW EXCLUSIVE MODE;
CREATE TEMP TABLE e08_reviewed_aliases (
    alias_key TEXT PRIMARY KEY,
    book_id BIGINT NOT NULL,
    source_identity TEXT NOT NULL,
    target_language TEXT NOT NULL
) ON COMMIT DROP;
INSERT INTO e08_reviewed_aliases(alias_key,book_id,source_identity,target_language) VALUES
""" + values + ";\n" + """DO $e08_verify$
BEGIN
    IF EXISTS (
        SELECT 1 FROM e08_reviewed_aliases a
        LEFT JOIN books b ON b.id=a.book_id
         AND b.source_identity=a.source_identity
         AND b.target_language=a.target_language
        WHERE b.id IS NULL
    ) THEN
        RAISE EXCEPTION 'E-08: reviewed Book ID/source identity mismatch';
    END IF;
    IF EXISTS (
        SELECT 1 FROM e08_reviewed_aliases a
        JOIN book_legacy_mail_refs r ON r.alias_key=a.alias_key
        WHERE r.book_id<>a.book_id
    ) THEN
        RAISE EXCEPTION 'E-08: legacy alias is assigned to another Book';
    END IF;
END
$e08_verify$;
INSERT INTO book_legacy_mail_refs(alias_key,book_id)
SELECT alias_key,book_id FROM e08_reviewed_aliases
ON CONFLICT (alias_key) DO NOTHING;
COMMIT;
"""


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-export", required=True, type=Path)
    parser.add_argument("--reviewed-map", required=True, type=Path)
    parser.add_argument("--output-sql", type=Path, help="offline SQL output, not executed")
    parser.add_argument("--check-only", action="store_true", help="validate without writing SQL")
    args = parser.parse_args(argv)
    if not args.check_only and not args.output_sql:
        parser.error("pass --check-only or --output-sql")
    try:
        aliases, unsupported = prepare(args.source_export, args.reviewed_map)
        if args.output_sql and not args.check_only:
            args.output_sql.write_text(render_sql(aliases), encoding="utf-8")
    except (OSError, ImportErrorDetail) as error:
        print(f"E-08 alias import rejected: {error}", file=sys.stderr)
        return 2
    print(f"verified_aliases={len(aliases)} unsupported_refs={len(unsupported)}")
    for source_id, field, raw in unsupported:
        print(f"unsupported source={source_id} field={field} ref={raw!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
