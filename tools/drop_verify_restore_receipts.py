#!/usr/bin/env python3
"""Compare independently retained object manifest and restored PG receipts."""

import argparse
import csv
import hashlib
import hmac
from pathlib import Path
import re
import sys

from i11_object_manifest import (
    HEX_SHA256,
    MAX_OBJECTS,
    Refused,
    manifest_bytes,
    safe_key,
    validate_manifest,
)


def compare(manifest, expected_digest, receipts):
    if not HEX_SHA256.fullmatch(expected_digest):
        raise Refused("independent manifest SHA-256 required")
    raw = manifest_bytes(manifest)
    if not hmac.compare_digest(hashlib.sha256(raw).hexdigest(), expected_digest):
        raise Refused("independent manifest digest mismatch")
    objects = validate_manifest(raw)
    index = 0
    previous = ""
    unbacked = undeclared = changed = 0
    csv.field_size_limit(1024)
    for count, row in enumerate(csv.reader(receipts, strict=True), 1):
        if count > MAX_OBJECTS or len(row) != 3:
            raise Refused("invalid or oversized receipt export")
        key, size, digest = row
        safe_key(key)
        if key <= previous:
            raise Refused("PostgreSQL receipts contain duplicate or unsorted objects")
        previous = key
        if (not re.fullmatch(r"[1-9][0-9]{0,18}", size)
                or not HEX_SHA256.fullmatch(digest)
                or digest == "0" * 64):
            raise Refused("invalid PostgreSQL size or SHA-256")
        while index < len(objects) and objects[index]["key"] < key:
            undeclared += 1
            index += 1
        if index == len(objects) or objects[index]["key"] > key:
            unbacked += 1
            continue
        if objects[index]["size"] != int(size) or objects[index]["sha256"] != digest:
            changed += 1
        index += 1
    undeclared += len(objects) - index
    if unbacked or undeclared or changed:
        raise Refused(
            f"receipt mismatch unbacked={unbacked} undeclared={undeclared} changed={changed}"
        )
    return len(objects)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--expected-manifest-sha256", required=True)
    args = parser.parse_args()
    try:
        total = compare(args.manifest, args.expected_manifest_sha256, sys.stdin)
    except (Refused, OSError, UnicodeError, csv.Error) as error:
        reason = str(error) if isinstance(error, Refused) else "unreadable or invalid input"
        print(f"TOR_DROP_RESTORE_RECEIPTS_REFUSED: {reason}", file=sys.stderr)
        return 2
    print(f"TOR_DROP_RESTORE_RECEIPTS_VERIFIED objects={total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
