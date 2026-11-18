#!/usr/bin/env python3
"""Verify staged replacement PACKs and optionally seal the source in PostgreSQL."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys

from i11_object_manifest import (
    HEX_SHA256, Refused, reject_duplicate_json_fields, root_directory, safe_key,
)


MAX_MEMBERS = 4096
MAX_PLAN_BYTES = 8 * 1024 * 1024
MAX_PACK_BYTES = 16 * 1024 * 1024
MAX_MANIFEST_BYTES = 1024 * 1024
PACK_KEY = re.compile(r"private/free-packs/[A-Za-z0-9_./-]+[.]zst\Z")
SEALED = b"DROP07_REPLACEMENT_SEALED"
QUERY_FILE = Path(__file__).with_name("drop07_pack_replacement_query.sql")


def integer(value, minimum=1, maximum=2**63 - 1):
    if type(value) is not int or not minimum <= value <= maximum:
        raise Refused("invalid numeric field in replacement plan")
    return value


def digest(value):
    if not isinstance(value, str) or not HEX_SHA256.fullmatch(value) or value == "0" * 64:
        raise Refused("invalid SHA-256 in replacement plan")
    return value


def load_plan(raw):
    if not raw or len(raw) > MAX_PLAN_BYTES:
        raise Refused("no complete bounded replacement plan")
    plan = json.loads(raw, object_pairs_hook=reject_duplicate_json_fields)
    if not isinstance(plan, dict) or set(plan) != {"source_pack_id", "replacements", "packs"}:
        raise Refused("invalid replacement plan")
    integer(plan["source_pack_id"])
    for rows in (plan["replacements"], plan["packs"]):
        if not isinstance(rows, list) or not 1 <= len(rows) <= MAX_MEMBERS:
            raise Refused("invalid replacement plan membership")
    return plan


def hash_staged_object(root, key, size, expected):
    path = root / safe_key(key)
    if path.resolve(strict=True) != path:
        raise Refused("symlinked replacement object refused")
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as file:
        before = os.fstat(file.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_size != size:
            raise Refused("staged replacement object size differs from PostgreSQL")
        hasher = hashlib.sha256()
        remaining = size + 1
        while remaining and (chunk := file.read(min(1024 * 1024, remaining))):
            hasher.update(chunk)
            remaining -= len(chunk)
        after = os.fstat(file.fileno())
    if remaining != 1 or after.st_mtime_ns != before.st_mtime_ns or hasher.hexdigest() != expected:
        raise Refused("staged replacement object bytes differ from PostgreSQL")


def verify(plan, root):
    targets = {}
    keys = set()
    for pack in plan["packs"]:
        pack_id = integer(pack["pack_id"])
        key = safe_key(pack["pack_key"])
        bucket = pack["bucket"]
        if (pack_id == plan["source_pack_id"] or pack_id in targets or key in keys
                or not PACK_KEY.fullmatch(key) or not 32 <= len(key) <= 240
                or pack["manifest_key"] != key[:-4] + ".manifest.json"
                or not isinstance(bucket, str) or bucket.startswith("public")
                or not re.fullmatch(r"[a-z][a-z0-9-]{1,61}[a-z0-9]", bucket)):
            raise Refused("invalid or duplicate replacement PACK identity")
        pack_size = integer(pack["pack_bytes"], 26, MAX_PACK_BYTES)
        sidecar_size = integer(pack["manifest_bytes"], 1, MAX_MANIFEST_BYTES)
        integer(pack["seek_table_start"], 1, pack_size - 25)
        hash_staged_object(root, key, pack_size, digest(pack["pack_sha256"]))
        hash_staged_object(root, pack["manifest_key"], sidecar_size, digest(pack["manifest_sha256"]))
        targets[pack_id] = pack
        keys.add(key)
    used = set()
    editions = set()
    for placement in plan["replacements"]:
        target = integer(placement["pack_id"])
        if target not in targets:
            raise Refused("active placement lacks verified replacement bytes")
        edition = tuple(integer(placement[field]) for field in
                        ("chapter_id", "version_id", "source_generation"))
        if edition in editions:
            raise Refused("duplicate source placement in replacement plan")
        editions.add(edition)
        used.add(target)
        integer(placement["active_generation"])
        integer(placement["size_bytes"], 1, 200_000)
        digest(placement["content_sha256"])
        offset = integer(placement["frame_offset"], 0)
        integer(placement["frame_bytes"], 1, min(204_096, targets[target]["seek_table_start"] - offset))
    if used != targets.keys():
        raise Refused("replacement plan contains unused PACKs")


def psql(pack_id, script):
    if not os.environ.get("PGSERVICE"):
        raise Refused("set PGSERVICE to the intended PostgreSQL service")
    command = ["psql", "-XAtq", "--no-password", "-v", "ON_ERROR_STOP=1",
               "-v", f"pack_id={integer(pack_id)}", "-f", "-"]
    completed = subprocess.run(command, input=script.encode("ascii"), capture_output=True,
                               timeout=120, env={**os.environ, "PGCONNECT_TIMEOUT": "10"})
    if completed.returncode:
        raise Refused("PostgreSQL operation refused or failed; inspect the source gate before retrying")
    return completed.stdout


def export(pack_id):
    query = QUERY_FILE.read_text(encoding="ascii")
    raw = psql(pack_id, f"BEGIN READ ONLY;\nSET LOCAL statement_timeout='25s';\n{query};\nCOMMIT;\n")
    return load_plan(raw)


def seal(plan):
    query = QUERY_FILE.read_text(encoding="ascii")
    expected = json.dumps(plan, ensure_ascii=True, separators=(",", ":")).encode("ascii").hex()
    script = f"""BEGIN ISOLATION LEVEL READ COMMITTED;
SET LOCAL statement_timeout='25s';
SELECT public.seal_replaced_free_pack_placements(:'pack_id'::bigint) AS can_seal
\\gset
\\if :can_seal
  WITH observed AS ({query})
  SELECT COALESCE((SELECT replacement_plan FROM observed) =
    convert_from(decode('{expected}','hex'),'UTF8')::jsonb,FALSE) AS matches
  \\gset
  \\if :matches
    COMMIT;
    \\echo {SEALED.decode('ascii')}
  \\else
    ROLLBACK;
  \\endif
\\else
  ROLLBACK;
\\endif
"""
    if psql(plan["source_pack_id"], script).strip() != SEALED:
        raise Refused("live replacement plan changed or is incomplete; source seal rolled back")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("export").add_argument("pack_id", type=int)
    check = commands.add_parser("verify")
    check.add_argument("plan", type=Path)
    check.add_argument("staged_root")
    check.add_argument("--seal", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "export":
            print(json.dumps(export(args.pack_id), ensure_ascii=True, separators=(",", ":")))
            return 0
        with args.plan.open("rb") as file:
            plan = load_plan(file.read(MAX_PLAN_BYTES + 1))
        verify(plan, root_directory(args.staged_root))
        if args.seal:
            seal(plan)
        status = SEALED.decode("ascii") if args.seal else "DROP07_REPLACEMENT_BYTES_MATCH"
        print(f"{status} source={plan['source_pack_id']} packs={len(plan['packs'])} "
              f"placements={len(plan['replacements'])}")
        return 0
    except (ValueError, KeyError, TypeError, OSError, RecursionError, subprocess.SubprocessError) as error:
        reason = str(error) if isinstance(error, Refused) else "unreadable or invalid replacement evidence"
        print(f"DROP07_REPLACEMENT_REFUSED: {reason}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
