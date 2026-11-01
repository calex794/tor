#!/usr/bin/env python3
"""Full read-only NEwBOO YDB taxonomy exporter; manual use, no deployment."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import sys
import tempfile
from uuid import UUID
from newboo_taxonomy_bridge import SOURCE, load

MAX_ROWS = 100000

def ident(prefix, table):
    if table not in ("book_taxonomies", "book_taxonomy_aliases"):
        raise ValueError("unauthorized source table")
    if prefix:
        pieces = prefix.strip("/").split("/")
        if "//" in prefix or not all(re.fullmatch(r"[A-Za-z0-9_-]+", p) for p in pieces):
            raise ValueError("invalid YDB path prefix")
        table = "/".join(pieces + [table])
    return chr(96) + table + chr(96)

def validate_target(endpoint, database, confirmation):
    if not re.fullmatch(r"grpcs?://[A-Za-z0-9_.-]+(?::[0-9]{2,5})?", endpoint):
        raise ValueError("YDB endpoint must be explicitly set")
    if not database.startswith("/") or any(
        piece in ("", ".", "..") for piece in database[1:].split("/")
    ):
        raise ValueError("invalid YDB database path")
    if database != confirmation:
        raise ValueError("--confirm-database mismatch")

def stream(tx, sql, keys, *, commit=False):
    data = []
    with tx.execute(sql, commit_tx=commit) as batches:
        for batch in batches:
            for row in batch.rows:
                item = {}
                for key in keys:
                    value = getattr(row, key)
                    if isinstance(value, UUID):
                        value = str(value)
                    if not isinstance(value, (str, int, type(None))):
                        raise ValueError("unexpected source data type")
                    if isinstance(value, str) and len(value) > 256:
                        raise ValueError("source value exceeds safety cap")
                    item[key] = value
                data.append(item)
                if len(data) > MAX_ROWS:
                    raise ValueError("source table exceeds safety cap")
    return data

def collect(session, ydb, prefix):
    """All four SELECT queries share one QuerySnapshotReadOnly transaction."""
    terms = ident(prefix, "book_taxonomies")
    aliases = ident(prefix, "book_taxonomy_aliases")
    tx = session.transaction(ydb.QuerySnapshotReadOnly())
    n1 = stream(tx, f"SELECT COUNT(*) AS total_rows FROM {terms};", ("total_rows",))
    n2 = stream(tx, f"SELECT COUNT(*) AS total_rows FROM {aliases};", ("total_rows",))
    if len(n1) != 1 or len(n2) != 1:
        raise ValueError("count query failed")
    counts = (n1[0]["total_rows"], n2[0]["total_rows"])
    if any(type(n) is not int or not 0 <= n <= MAX_ROWS for n in counts):
        raise ValueError("bad source count")
    rows1 = stream(tx, f"SELECT taxonomy_id,taxonomy_type,title,status FROM {terms} ORDER BY taxonomy_id;",
                   ("taxonomy_id", "taxonomy_type", "title", "status"))
    rows2 = stream(tx, f"SELECT alias_id,taxonomy_id,taxonomy_type,alias FROM {aliases} ORDER BY alias_id;",
                   ("alias_id", "taxonomy_id", "taxonomy_type", "alias"), commit=True)
    if (len(rows1), len(rows2)) != counts:
        raise ValueError("YDB streamed row count differs from snapshot counts")
    return {"source": SOURCE,
            "expected_counts": {"taxonomies": counts[0], "aliases": counts[1]},
            "taxonomies": rows1, "aliases": rows2,
            "capture": {"mode": "query-snapshot-readonly",
                        "timestamp_utc": datetime.now(timezone.utc).isoformat()}}

def retrieve(ydb, endpoint, database, prefix):
    # SDK credentials exclusively from environment; do not pass tokens as args.
    with ydb.Driver(endpoint=endpoint, database=database,
                    credentials=ydb.credentials_from_env_variables()) as driver:
        driver.wait(timeout=15, fail_fast=True)
        with ydb.QuerySessionPool(driver) as pool:
            return pool.retry_operation_sync(lambda s: collect(s, ydb, prefix))

def save(path, data):
    destination = Path(path).resolve()
    if destination.exists():
        raise ValueError("existing snapshot may not be overwritten")
    encoded = (json.dumps(data, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    if len(encoded) > 25000000:
        raise ValueError("exceeds TOR snapshot consumer limit")
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", dir=destination.parent, delete=False,
            prefix=".newboo-ydb-snapshot-", suffix=".json",
        ) as out:
            temporary = Path(out.name)
            out.write(encoded)
        load(temporary)
        if destination.exists():
            raise ValueError("existing snapshot may not be overwritten")
        os.rename(temporary, destination)
        temporary = None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)

def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--endpoint", default=os.environ.get("NEWBOO_YDB_ENDPOINT", ""))
    p.add_argument("--database", default=os.environ.get("NEWBOO_YDB_DATABASE", ""))
    p.add_argument("--table-prefix", default=os.environ.get("NEWBOO_YDB_TABLE_PREFIX", ""))
    p.add_argument("--confirm-database", required=True)
    p.add_argument("--out", required=True)
    a = p.parse_args(argv)
    try:
        validate_target(a.endpoint, a.database, a.confirm_database)
        ident(a.table_prefix, "book_taxonomies")
        ident(a.table_prefix, "book_taxonomy_aliases")
        if Path(a.out).exists():
            raise ValueError("existing snapshot may not be overwritten")
        try:
            import ydb
        except ImportError as e:
            raise RuntimeError("YDB Python SDK ydb==3.30.0 is required") from e
        data = retrieve(ydb, a.endpoint, a.database, a.table_prefix)
        save(a.out, data)
        counts = data["expected_counts"]
        print(f"Saved snapshot with {counts['taxonomies']} terms and {counts['aliases']} aliases")
        return 0
    except (ValueError, OSError, RuntimeError) as exc:
        # Never print full SDK errors/connection strings/tokens.
        print(f"YDB taxonomy snapshot failed: {type(exc).__name__}", file=sys.stderr)
        return 2

if __name__ == "__main__":
    raise SystemExit(main())
