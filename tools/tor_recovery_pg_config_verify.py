#!/usr/bin/env python3
"""RECOVERY-09a: verify completed PostgreSQL configuration, read-only.

No startup, subprocess, WAL retrieval, SQL, promotion or acceptance. Must
be repeated inside the eventual launcher's stage lock before any execution.
"""
from __future__ import annotations

import argparse
import os
import sys

from tor_recovery_host import Refused
from tor_recovery_pg import STAGE_RECEIPT, checked_directory
from tor_recovery_pg_prestart import (
    ROOT, MAX_CONFIG, add_preflight_arguments, preflight, private_read,
    validate_preflight_host,
)
from tor_recovery_pg_startup_config import (
    ATTEMPT, CONFIG_RECEIPT, ORIGINAL_AUTO, STARTUP_DIR, TEMP_AUTO,
    add_resource_arguments, config_receipt_bytes, render_startup, stage_lock,
)


def verify_prepared_files(root, args, pg_uid, pg_gid,
                          operator_uid=0, operator_gid=0):
    """Caller has validated the host and rerun preflight under stage_lock."""
    expected = render_startup(root, args)
    evidence = root / "evidence"
    pgdata = root / "pgdata"
    startup = root / "config" / STARTUP_DIR
    checked_directory(startup, operator_uid, pg_gid, 0o750)
    if {entry.name for entry in startup.iterdir()} != set(expected):
        raise Refused("unexpected trusted startup configuration inventory")
    for name, content in expected.items():
        actual = private_read(startup / name, operator_uid, pg_gid,
                              (0o640,), MAX_CONFIG)
        if actual != content:
            raise Refused("prepared configuration differs from approved inputs")
    if os.path.lexists(pgdata / TEMP_AUTO):
        raise Refused("partial auto.conf replacement exists")
    # PG always reads auto.conf even with an explicit config_file. It must
    # still be an empty ordinary postgres-owned file, not merely hash-pinned.
    private_read(pgdata / "postgresql.auto.conf", pg_uid, pg_gid, (0o600,), 0)
    attempted = private_read(evidence / ATTEMPT,
                             operator_uid, operator_gid, (0o600,), 128)
    if attempted != b"RECOVERY-08 attempted\n":
        raise Refused("invalid configuration attempt marker")
    original = private_read(evidence / ORIGINAL_AUTO,
                            operator_uid, operator_gid, (0o600,), MAX_CONFIG)
    stage = private_read(evidence / STAGE_RECEIPT,
                         operator_uid, operator_gid, (0o600,), 16384)
    receipt = private_read(evidence / CONFIG_RECEIPT,
                           operator_uid, operator_gid, (0o600,), 16384)
    # Compare exact canonical bytes instead of trusting digests supplied
    # by the receipt. All expected settings come from the repeated selection.
    if receipt != config_receipt_bytes(args, stage, original, expected):
        raise Refused("configuration receipt differs from verified files")


def verify(args):
    uid, gid = validate_preflight_host(args)
    with stage_lock(ROOT):
        preflight(args)
        verify_prepared_files(ROOT, args, uid, gid)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_preflight_arguments(parser)
    add_resource_arguments(parser)
    args = parser.parse_args()
    try:
        verify(args)
    except (Refused, OSError, KeyError, ValueError, UnicodeError) as error:
        reason = str(error) if isinstance(error, Refused) else "local config verification failed"
        print("TOR_RECOVERY_PG_CONFIG_VERIFY_REFUSED: " + reason, file=sys.stderr)
        return 2
    print("TOR_RECOVERY_PG_CONFIG_VERIFIED "
          "startup_authorized=false postgres_started=false "
          "wal_replayed=false recovery_accepted=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
