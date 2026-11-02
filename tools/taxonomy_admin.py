#!/usr/bin/env python3
"""Offline migrator-only taxonomy curator: reviewed terms, aliases, reconciliation.

No HTTP admin rights or application DB role are accepted. Mutations require
explicit --confirm-db; psql variables use :'var' SQL literal escaping.
"""
import argparse
import os
import re
import subprocess
import sys
from urllib.parse import unquote, urlsplit

READ_ONLY = """
-- This queue uses PERF-233's transactionally maintained *public* projection.
-- "uses" sums per-spelling references; multiple equivalent spellings on one
-- book can make it larger than the count of distinct books.
WITH facets AS (
 SELECT kind,name,admin_taxonomy_key(name) AS k,reference_count
 FROM public_catalog_facets
 WHERE reference_count>0
), unmatched AS (
 SELECT f.kind,f.k,
        sum(f.reference_count)::bigint AS uses,
        count(*)::bigint AS spellings,
        min(f.name) AS example_label
 FROM facets f
 WHERE NOT EXISTS (
  SELECT 1 FROM book_taxonomy_terms t
  WHERE t.kind=f.kind AND t.status='active'
    AND (t.search_key=f.k OR EXISTS (
     SELECT 1 FROM book_taxonomy_aliases a
     WHERE a.kind=f.kind AND a.search_key=f.k AND a.term_id=t.id
    ))
 )
 GROUP BY f.kind,f.k
)
SELECT u.kind,
       coalesce(t.label,u.example_label) AS review_label,
       u.k AS normalized_key,
       u.uses,u.spellings,
       t.id AS term_id,
       coalesce(t.status,'not_staged') AS status
FROM unmatched u
LEFT JOIN book_taxonomy_terms t
  ON t.kind=u.kind AND t.search_key=u.k
ORDER BY u.uses DESC,u.kind,u.k
LIMIT :'limit'::integer OFFSET :'offset'::integer;
"""

VARIANTS_SQL = """
-- Exact normalized key: Unicode symbols do not become LIKE wildcards.
-- JSON Lines preserve raw Unicode labels containing separators or newlines.
WITH prefix AS (
 SELECT admin_taxonomy_key(:'label') AS k
)
SELECT json_build_object(
    'kind',f.kind,'label',f.name,'reference_count',f.reference_count,
    'normalized_key',p.k
)::text
FROM public_catalog_facets f CROSS JOIN prefix p
WHERE f.kind=:'kind' AND f.reference_count>0
  AND admin_taxonomy_key(f.name)=p.k
ORDER BY f.reference_count DESC,f.name
LIMIT :'limit'::integer OFFSET :'offset'::integer;
"""

LOOKUP_SQL = """
-- Read-only indexed prefix lookup of canonical terms and approved aliases.
WITH prefix AS (
 SELECT admin_taxonomy_key(:'query') AS k
), matches AS (
 SELECT t.id,0 AS match_rank FROM book_taxonomy_terms t CROSS JOIN prefix p
 WHERE t.kind=:'kind' AND t.search_key LIKE p.k || '%'
   -- NFKC converts fullwidth punctuation into LIKE wildcard characters.
   AND strpos(p.k,'%')=0 AND strpos(p.k,'_')=0
   AND strpos(p.k,chr(92))=0
 UNION ALL
 SELECT a.term_id AS id,1 AS match_rank
 FROM book_taxonomy_aliases a CROSS JOIN prefix p
 WHERE a.kind=:'kind' AND a.search_key LIKE p.k || '%'
   AND strpos(p.k,'%')=0 AND strpos(p.k,'_')=0
   AND strpos(p.k,chr(92))=0
), ranked AS (
 SELECT id,min(match_rank) AS match_rank FROM matches GROUP BY id
)
SELECT t.id,t.kind,t.label,t.status,
       CASE WHEN r.match_rank=0 THEN 'canonical' ELSE 'alias' END
FROM ranked r JOIN book_taxonomy_terms t ON t.id=r.id
ORDER BY r.match_rank,t.label,t.id LIMIT :'limit'::integer;
"""

def database_name(url: str) -> str:
    p = urlsplit(url)
    name = unquote(p.path.removeprefix("/"))
    if p.scheme not in {"postgres", "postgresql"} or unquote(p.username or "") != "tor_migrator":
        raise ValueError("TOR_TAXONOMY_ADMIN_URL must authenticate as tor_migrator")
    if not p.hostname or not re.fullmatch(r"[a-zA-Z][a-zA-Z0-9_]{1,62}", name):
        raise ValueError("invalid migration-owner URL or database name")
    return name

def run_sql(url: str, sql: str, **values: str) -> str:
    args = ["psql", "-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "-d", url]
    for key, value in values.items():
        if not re.fullmatch(r"[a-z_]+", key):
            raise ValueError("invalid bind identifier")
        args.extend(["-v", f"{key}={value}"])
    done = subprocess.run(
        args, input=sql, text=True, capture_output=True, check=False,
        timeout=30, env=os.environ.copy(),
    )
    if done.returncode:
        # Do not echo passwords or the command line.
        raise RuntimeError(done.stderr.strip()[-1200:] or "psql operation failed")
    return done.stdout.strip()

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--confirm-db", help="type the target database name for any write")
    commands = parser.add_subparsers(dest="command", required=True)
    add = commands.add_parser("add", help="approve a new canonical term")
    add.add_argument("kind", choices=("genre", "tag", "fandom"))
    add.add_argument("label")
    add.add_argument("--curator", required=True)
    alias = commands.add_parser("alias", help="approve a synonym for one active term")
    alias.add_argument("kind", choices=("genre", "tag", "fandom"))
    alias.add_argument("term_id", type=int)
    alias.add_argument("label")
    alias.add_argument("--curator", required=True)
    reconcile = commands.add_parser("reconcile", help="review/apply one complete legacy book")
    reconcile.add_argument("book_id", type=int)
    reconcile.add_argument("--curator", required=True)
    reconcile.add_argument("--apply", action="store_true")
    listing = commands.add_parser("unmapped", help="public-only legacy unknown names")
    listing.add_argument("--limit", type=int, default=40)
    listing.add_argument("--offset", type=int, default=0, help="skip earlier unmatched rows")
    variants = commands.add_parser("variants", help="list public raw spellings for one normalized key")
    variants.add_argument("kind", choices=("genre", "tag", "fandom"))
    variants.add_argument("label", help="exact key or sample label from unmapped queue")
    variants.add_argument("--limit", type=int, default=50)
    variants.add_argument("--offset", type=int, default=0)
    lookup = commands.add_parser("lookup", help="find canonical IDs by name or alias")
    lookup.add_argument("kind", choices=("genre", "tag", "fandom"))
    lookup.add_argument("query", help="indexed, normalized prefix")
    lookup.add_argument("--limit", type=int, default=40)
    args = parser.parse_args(argv)
    url = os.environ.get("TOR_TAXONOMY_ADMIN_URL", "")
    try:
        dbname = database_name(url)
        if args.command == "unmapped":
            if not 1 <= args.limit <= 500:
                raise ValueError("limit must be 1..500")
            if not 0 <= args.offset <= 1_000_000:
                raise ValueError("offset must be 0..1000000")
            print(run_sql(url, READ_ONLY, limit=str(args.limit), offset=str(args.offset)))
            return 0
        if args.command == "variants":
            if not 1 <= args.limit <= 500:
                raise ValueError("variants limit must be 1..500")
            if not 0 <= args.offset <= 1_000_000:
                raise ValueError("variants offset must be 0..1000000")
            if (not 1 <= len(args.label) <= 80 or args.label != args.label.strip()
                    or any(ord(c) < 32 or ord(c) == 127 for c in args.label)):
                raise ValueError("variants label must be a 1..80 character literal")
            print(run_sql(
                url, VARIANTS_SQL, kind=args.kind, label=args.label,
                limit=str(args.limit), offset=str(args.offset),
            ))
            return 0
        if args.command == "lookup":
            if not 1 <= args.limit <= 100:
                raise ValueError("lookup limit must be 1..100")
            q = args.query
            if (not 1 <= len(q) <= 80 or q != q.strip()
                    or any(ord(c) < 32 or ord(c) in (92, 127)
                           or c in "%_" for c in q)):
                raise ValueError("lookup needs a 1..80 character literal prefix without wildcards")
            print(run_sql(url, LOOKUP_SQL, kind=args.kind, query=q, limit=str(args.limit)))
            return 0
        if args.command == "reconcile" and not args.apply:
            if args.book_id <= 0:
                raise ValueError("book_id must be positive")
            print(run_sql(url, """
                SELECT kind,label FROM (
                  SELECT 'genre' AS kind,unnest(genres) AS label FROM books WHERE id=:'id'::bigint
                  UNION ALL SELECT 'tag',unnest(tags) FROM books WHERE id=:'id'::bigint
                  UNION ALL SELECT 'fandom',unnest(fandoms) FROM books WHERE id=:'id'::bigint
                ) old_labels ORDER BY kind,label;
            """, id=str(args.book_id)))
            print("READ-ONLY PREVIEW; no changes applied")
            return 0
        if args.confirm_db != dbname:
            raise ValueError(f"write requires --confirm-db {dbname}")
        if args.command == "add":
            print(run_sql(url,
                "SELECT admin_taxonomy_add_term(:'kind',:'label',:'curator');",
                kind=args.kind, label=args.label, curator=args.curator))
        elif args.command == "alias":
            if args.term_id <= 0:
                raise ValueError("term_id must be positive")
            run_sql(url,
                "SELECT admin_taxonomy_add_alias(:'kind',:'term_id'::bigint,:'label',:'curator');",
                kind=args.kind, term_id=str(args.term_id),
                label=args.label, curator=args.curator)
            print("Alias accepted")
        else:
            if args.book_id <= 0:
                raise ValueError("book_id must be positive")
            print(run_sql(url,
                "SELECT admin_taxonomy_reconcile_book(:'id'::bigint,:'curator');",
                id=str(args.book_id), curator=args.curator))
        return 0
    except (ValueError, RuntimeError, subprocess.TimeoutExpired, OSError) as err:
        print(f"taxonomy curator: {err}", file=sys.stderr)
        return 2

if __name__ == "__main__":
    raise SystemExit(main())
