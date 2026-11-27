#!/usr/bin/env python3
"""Estimate matching registered obsolete SINGLE bytes using two pinned manifests.

Inputs are private, independently acquired snapshots of the same source scope.
No storage network calls, no PG writes, no seal, no deletion authorization.
"""

import argparse
import csv
import json
from pathlib import Path
import sys

from drop_verify_restore_receipts import (
    load_retained_manifest,
    receipt_rows,
    validate_single_receipt,
)
from i11_object_manifest import Refused


CATEGORIES = (
    "matched",
    "primary_missing",
    "primary_changed",
    "backup_missing",
    "backup_changed",
)


def classify(objects, index, key, size, digest):
    while index < len(objects) and objects[index]["key"] < key:
        index += 1
    if index == len(objects) or objects[index]["key"] != key:
        return "missing", index
    obj = objects[index]
    if (obj["size"], obj["sha256"]) != (size, digest):
        return "changed", index
    return "matched", index


def preview(receipts, primary_manifest, primary_digest, backup_manifest, backup_digest):
    primary = load_retained_manifest(primary_manifest, primary_digest)
    backup = load_retained_manifest(backup_manifest, backup_digest)
    result = {name: {"objects": 0, "registered_bytes": 0} for name in CATEGORIES}
    primary_index = backup_index = 0
    count = total_bytes = 0
    for key, size, digest in receipt_rows(receipts):
        validate_single_receipt(key, size)
        primary_state, primary_index = classify(primary, primary_index, key, size, digest)
        backup_state, backup_index = classify(backup, backup_index, key, size, digest)
        if primary_state != "matched":
            name = "primary_" + primary_state
        elif backup_state != "matched":
            name = "backup_" + backup_state
        else:
            name = "matched"
        result[name]["objects"] += 1
        result[name]["registered_bytes"] += size
        count += 1
        total_bytes += size
    return {
        "mode": "snapshot_preview_only",
        "deletion_authorized": False,
        "candidates": {"objects": count, "registered_bytes": total_bytes},
        "dispositions": result,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--primary-manifest", type=Path, required=True)
    parser.add_argument("--expected-primary-manifest-sha256", required=True)
    parser.add_argument("--backup-manifest", type=Path, required=True)
    parser.add_argument("--expected-backup-manifest-sha256", required=True)
    args = parser.parse_args()
    try:
        report = preview(
            sys.stdin, args.primary_manifest, args.expected_primary_manifest_sha256,
            args.backup_manifest, args.expected_backup_manifest_sha256,
        )
    except (Refused, OSError, UnicodeError, csv.Error) as error:
        reason = str(error) if isinstance(error, Refused) else "unreadable or invalid input"
        print(f"D001_SINGLE_SPACE_PREVIEW_REFUSED: {reason}", file=sys.stderr)
        return 2
    # Aggregate only. Do not emit private object names or ready-to-delete keys.
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
