#!/usr/bin/env python3
"""Compare independently retained object manifest and restored PG receipts."""

import argparse
import csv
import hashlib
import hmac
from itertools import islice
from pathlib import Path
import re
import sys

from i11_object_manifest import (
    HEX_SHA256,
    MAX_OBJECTS,
    Refused,
    inventory,
    manifest_bytes,
    root_directory,
    safe_key,
    validate_manifest,
)


def load_retained_manifest(manifest, expected_digest):
    if not HEX_SHA256.fullmatch(expected_digest):
        raise Refused("independent manifest SHA-256 required")
    raw = manifest_bytes(manifest)
    if not hmac.compare_digest(hashlib.sha256(raw).hexdigest(), expected_digest):
        raise Refused("independent manifest digest mismatch")
    return validate_manifest(raw)


def receipt_rows(receipts):
    previous = ""
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
        yield key, int(size), digest


def compare(manifest, expected_digest, receipts):
    objects = load_retained_manifest(manifest, expected_digest)
    index = 0
    unbacked = undeclared = changed = 0
    for key, size, digest in receipt_rows(receipts):
        while index < len(objects) and objects[index]["key"] < key:
            undeclared += 1
            index += 1
        if index == len(objects) or objects[index]["key"] > key:
            unbacked += 1
            continue
        if objects[index]["size"] != size or objects[index]["sha256"] != digest:
            changed += 1
        index += 1
    undeclared += len(objects) - index
    if unbacked or undeclared or changed:
        raise Refused(
            f"receipt mismatch unbacked={unbacked} undeclared={undeclared} changed={changed}"
        )
    return len(objects)


def compare_restored_pack(manifest, expected_digest, receipts, restored_root):
    rows = list(islice(receipt_rows(receipts), 3))
    if len(rows) != 2:
        raise Refused("expected exactly two PACK receipts")
    (sidecar_key, sidecar_size, _), (pack_key, pack_size, _) = rows
    stem = pack_key.removesuffix(".zst")
    if (not pack_key.startswith("private/free-packs/")
            or stem == pack_key or stem.endswith("/") or sidecar_key != stem + ".manifest.json"
            or not 26 <= pack_size <= 16 * 1024 * 1024 or not 1 <= sidecar_size <= 1024 * 1024):
        raise Refused("invalid PACK or manifest pair")

    return compare_restored_selection(
        manifest, expected_digest, rows, restored_root, "PACK or manifest",
    )


def compare_restored_single(manifest, expected_digest, receipts, restored_root):
    rows = list(islice(receipt_rows(receipts), 2))
    if len(rows) != 1:
        raise Refused("expected exactly one SINGLE receipt")
    key, size, _ = rows[0]
    if (not re.fullmatch(r"private/free-chapters/[1-9][0-9]{0,18}/[1-9][0-9]{0,18}[.]md", key)
            or not 1 <= size <= 200_000):
        raise Refused("invalid FREE SINGLE receipt")
    return compare_restored_selection(
        manifest, expected_digest, rows, restored_root, "SINGLE",
    )


def compare_restored_selection(manifest, expected_digest, rows, restored_root, label):
    expected = {key: (size, digest) for key, size, digest in rows}
    retained = load_retained_manifest(manifest, expected_digest)
    backed = {obj["key"]: (obj["size"], obj["sha256"]) for obj in retained
              if obj["key"] in expected}
    if backed != expected:
        raise Refused(f"{'PACK' if label == 'PACK or manifest' else label} receipts differ from the retained manifest")
    actual = inventory(restored_root, expected_sizes={key: size for key, size, _ in rows})
    if {obj["key"]: (obj["size"], obj["sha256"]) for obj in actual} != expected:
        raise Refused(f"restored {label} bytes mismatch")
    return len(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--expected-manifest-sha256", required=True)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument(
        "--restored-pack-root",
        help="verify only the exact PACK and sidecar staged under this directory",
    )
    selection.add_argument(
        "--restored-single-root",
        help="verify only one exact FREE SINGLE staged under this directory",
    )
    args = parser.parse_args()
    try:
        if args.restored_single_root is not None:
            total = compare_restored_single(
                args.manifest, args.expected_manifest_sha256, sys.stdin,
                root_directory(args.restored_single_root),
            )
            result = "TOR_D001_SINGLE_BACKUP_VERIFIED"
        elif args.restored_pack_root is None:
            total = compare(args.manifest, args.expected_manifest_sha256, sys.stdin)
            result = "TOR_DROP_RESTORE_RECEIPTS_VERIFIED"
        else:
            total = compare_restored_pack(
                args.manifest, args.expected_manifest_sha256, sys.stdin,
                root_directory(args.restored_pack_root),
            )
            result = "TOR_DROP_PACK_BACKUP_VERIFIED"
    except (Refused, OSError, UnicodeError, csv.Error) as error:
        reason = str(error) if isinstance(error, Refused) else "unreadable or invalid input"
        print(f"TOR_DROP_RESTORE_RECEIPTS_REFUSED: {reason}", file=sys.stderr)
        return 2
    print(f"{result} objects={total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
