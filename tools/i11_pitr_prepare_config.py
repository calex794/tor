#!/usr/bin/env python3
"""Use real recovery configuration helpers in the existing disposable PITR fixture.

Never calls an operator entrypoint, reads external credentials, starts a
process or accepts operational recovery. Only fixture-owned files under
the fixed network-free container's random temporary workspace.
"""
from __future__ import annotations

from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import sys
from types import SimpleNamespace

from tor_recovery_host import Refused
from tor_recovery_pg import STAGE_RECEIPT, checked_directory, checkpoint
from tor_recovery_pg_prestart import (
    MAX_CONFIG, private_read, verify_data_tree, verify_stage_receipt,
)
from tor_recovery_pg_startup_config import (
    prepare_files, sha, stage_lock, sync_directory, write_exclusive,
)
from tor_recovery_pg_config_verify import verify_prepared_files


def prepare(work, label, target, timeline):
    uid, gid = os.geteuid(), os.getegid()
    if (os.environ.get("TOR_I11_DISPOSABLE_CONTAINER") != "yes"
            or not Path("/.dockerenv").is_file() or uid == 0 or gid == 0
            or not re.fullmatch(r"/tmp/tor-i11-pitr\.[A-Za-z0-9]{8}", str(work))
            or work.resolve(strict=True) != work):
        raise Refused("requires unprivileged disposable PITR fixture")
    checked_directory(work, uid, gid, 0o700)
    if os.path.lexists(work / "source"):
        raise Refused("fixture source must already be removed")
    if not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}\+00", target):
        raise Refused("invalid fixture UTC target")
    target_utc = target.replace(" ", "T").replace("+00", "Z")
    incident = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    checkpoint(label, target_utc, incident, timeline)
    args = SimpleNamespace(
        backup_label=label, target_utc=target_utc, incident_utc=incident,
        timeline=timeline, pg_port=15435,
        max_connections=16, max_prepared_transactions=0,
        max_locks_per_transaction=64, max_wal_senders=10,
        max_worker_processes=8, shared_buffers_mb=32,
    )
    root = work / "time-recovery"
    checked_directory(root, uid, gid, 0o700)
    checked_directory(root / "config", uid, gid, 0o750)
    checked_directory(root / "evidence", uid, gid, 0o700, empty=True)
    checked_directory(root / "pgsocket", uid, gid, 0o700, empty=True)
    checked_directory(root / "pglog", uid, gid, 0o700)
    checked_directory(root / "tablespaces", uid, gid, 0o700, empty=True)
    verify_data_tree(root / "pgdata", uid, gid)
    repository = private_read(root / "config/pgbackrest-recovery.conf",
                              uid, gid, (0o640,), MAX_CONFIG)
    # A synthetic local POSIX receipt is explicitly NOT an operational
    # S3 stage receipt. The real verification/prepare helpers are shared.
    selection = {
        "schema_version": 1, "scope": "isolated_container_fixture_only",
        "backup_label": label, "target_utc": target_utc,
        "incident_utc": incident, "timeline": timeline, "pg_port": args.pg_port,
        "repository_config_sha256": sha(repository),
    }
    evidence = root / "evidence"
    write_exclusive(evidence / "pg-restore.lock", b"", uid, gid, 0o600)
    write_exclusive(evidence / STAGE_RECEIPT,
                    (json.dumps(selection, sort_keys=True) + "\n").encode("ascii"),
                    uid, gid, 0o600)
    sync_directory(evidence)
    with stage_lock(root, uid, gid):
        verify_stage_receipt(root, selection, uid, gid, gid)
        prepare_files(root, args, uid, gid, uid, gid)
        verify_data_tree(root / "pgdata", uid, gid)
        verify_stage_receipt(root, selection, uid, gid, gid)
        verify_prepared_files(root, args, uid, gid, uid, gid)


def main():
    if len(sys.argv) != 5:
        return 2
    try:
        prepare(Path(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4]))
    except (Refused, OSError, ValueError):
        # No inherited configuration, secret, path or database log is printed.
        print("I11_PITR_GENERATED_CONFIG_REFUSED", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
