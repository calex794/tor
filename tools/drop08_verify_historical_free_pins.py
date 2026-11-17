#!/usr/bin/env python3
"""Fail-closed offline check of restored Layer/saved FREE pin source receipts.

This never proves actual S3 bytes, ACL or independent recovery. Validate every
registered object with DROP-08a and exercise reader rights separately.
"""
from __future__ import annotations

import csv
import re
import sys

MAX_ROWS = 1_000_000
POSITIVE_ID = re.compile(r"[1-9][0-9]{0,18}\Z")


class Refused(ValueError):
    pass


def verify(rows):
    previous = None
    counts = {"layer": 0, "saved": 0}
    missing = 0
    csv.field_size_limit(128)
    for count, row in enumerate(csv.reader(rows, strict=True), 1):
        if count > MAX_ROWS or len(row) != 7:
            raise Refused("invalid or oversized PostgreSQL export")
        kind, chapter, version, valid_member, pg_body, single, pack = row
        if kind not in counts or not POSITIVE_ID.fullmatch(chapter) or not POSITIVE_ID.fullmatch(version):
            raise Refused("invalid historical edition identity")
        current = (kind, int(chapter), int(version))
        if previous is not None and current <= previous:
            raise Refused("duplicate or unsorted historical edition export")
        previous = current
        if any(flag not in {"0", "1"} for flag in (valid_member, pg_body, single, pack)):
            raise Refused("invalid physical source evidence")
        if valid_member != "1":
            raise Refused("invalid historical chapter-version or Layer book membership")
        counts[kind] += 1
        if not (pg_body == "1" or single == "1" or pack == "1"):
            missing += 1
    if missing:
        raise Refused(f"missing_source_receipts={missing}")
    if not counts["layer"] or not counts["saved"]:
        raise Refused("DR acceptance requires exercised saved and Layer FREE pins")
    return counts


def main():
    try:
        totals = verify(sys.stdin)
    except (Refused, csv.Error, UnicodeError, OverflowError) as error:
        reason = str(error) if isinstance(error, Refused) else "invalid PostgreSQL export"
        print("TOR_DROP08_HISTORICAL_PINS_REFUSED: " + reason, file=sys.stderr)
        return 2
    print("TOR_DROP08_HISTORICAL_PINS_METADATA_OK "
          f"layer={totals['layer']} saved={totals['saved']} "
          "external_restore_and_rights_still_unverified=true")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
