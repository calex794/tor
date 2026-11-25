#!/usr/bin/env python3
"""RECOVERY-07: read-only preflight of the selected staged PG18 backup.

No startup, config changes, WAL retrieval, SQL, promotion or recovery acceptance.
Reject unsupported layouts instead of following paths from a restored archive.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import pwd
import stat
import sys

from tor_recovery_host import NAME, PARENT, Refused, reject_live_markers, validate_target
from tor_recovery_pg import (
    STAGE_RECEIPT, checked_directory, checkpoint, read_repository,
    stage_receipt, unique_fields,
)

ROOT = PARENT / NAME
MAX_CONFIG = 256 * 1024


def private_read(path, uid, gid, modes, limit):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as handle:
        info = os.fstat(handle.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
                or (info.st_uid, info.st_gid) != (uid, gid)
                or stat.S_IMODE(info.st_mode) not in modes
                or info.st_size > limit):
            raise Refused("unsafe or oversized recovery evidence/config file")
        raw = handle.read(limit + 1)
    if len(raw) > limit:
        raise Refused("oversized recovery evidence/config file")
    return raw


def verify_stage_receipt(root, expected, operator_uid, operator_gid, pg_gid):
    evidence = private_read(
        root / "evidence" / STAGE_RECEIPT,
        operator_uid, operator_gid, (0o600,), 16384,
    )
    try:
        actual = json.loads(evidence, object_pairs_hook=unique_fields)
    except (ValueError, UnicodeError) as error:
        raise Refused("invalid stage receipt") from error
    # Canonical serialization also distinguishes JSON true from integer 1.
    if json.dumps(actual, sort_keys=True) != json.dumps(expected, sort_keys=True):
        raise Refused("stage receipt differs from independently selected checkpoint")
    config = private_read(
        root / "config" / "pgbackrest-recovery.conf",
        operator_uid, pg_gid, (0o640,), MAX_CONFIG,
    )
    if hashlib.sha256(config).hexdigest() != expected["repository_config_sha256"]:
        raise Refused("staged repository configuration changed")
    checked_directory(root / "config" / "pgbackrest-empty",
                      operator_uid, pg_gid, 0o750, empty=True)


def verify_data_tree(pgdata, uid, gid):
    checked_directory(pgdata, uid, gid, 0o700)
    device = pgdata.lstat().st_dev
    pending = [pgdata]
    while pending:
        current = pending.pop()
        with os.scandir(current) as entries:
            for entry in entries:
                info = entry.stat(follow_symlinks=False)
                mode = stat.S_IMODE(info.st_mode)
                if ((info.st_uid, info.st_gid) != (uid, gid)
                        or info.st_dev != device):
                    raise Refused("unexpected restored file ownership or filesystem")
                if stat.S_ISDIR(info.st_mode) and mode == 0o700:
                    pending.append(Path(entry.path))
                elif (stat.S_ISREG(info.st_mode) and info.st_nlink == 1
                      and mode in (0o400, 0o600)):
                    continue
                else:
                    raise Refused("unsafe restored link, special file or permission")
    # No live/standby process markers; no implicit tablespace or WAL mapping.
    for name in ("postmaster.pid", "standby.signal"):
        if os.path.lexists(pgdata / name):
            raise Refused("unexpected active or standby cluster marker")
    checked_directory(pgdata / "global", uid, gid, 0o700)
    checked_directory(pgdata / "pg_wal", uid, gid, 0o700)
    checked_directory(pgdata / "pg_tblspc", uid, gid, 0o700, empty=True)
    if private_read(pgdata / "PG_VERSION", uid, gid, (0o400, 0o600), 16).strip() != b"18":
        raise Refused("staged cluster must be PostgreSQL 18")
    if private_read(pgdata / "recovery.signal", uid, gid, (0o400, 0o600), 0):
        raise Refused("unexpected recovery signal contents")
    control = pgdata / "global" / "pg_control"
    if not stat.S_ISREG(control.lstat().st_mode) or control.lstat().st_size == 0:
        raise Refused("missing staged PostgreSQL control file")
    # pgBackRest has normally generated recovery settings here. Inspect only
    # type and size, never execute, trust, print or modify inherited contents.
    private_read(pgdata / "postgresql.auto.conf", uid, gid, (0o400, 0o600), MAX_CONFIG)


def validate_preflight_host(args):
    checkpoint(args.backup_label, args.target_utc, args.incident_utc, args.timeline)
    if os.geteuid() != 0 or "GITHUB_ACTIONS" in os.environ:
        raise Refused("operator-only independent restore host required; CI prohibited")
    if not args.operator_confirm_quiescent_isolated_host:
        raise Refused("operator confirmation of quiescent isolated host required")
    machine = Path("/etc/machine-id").read_text(encoding="ascii").strip()
    validate_target(args.source_machine_id, args.target_machine_id, machine, args.pg_port)
    reject_live_markers()
    postgres = pwd.getpwnam("postgres")
    uid, gid = postgres.pw_uid, postgres.pw_gid
    if uid == 0 or gid == 0:
        raise Refused("unprivileged postgres account required")
    checked_directory(ROOT, 0, gid, 0o710)
    checked_directory(ROOT / "config", 0, gid, 0o750)
    checked_directory(ROOT / "evidence", 0, 0, 0o700)
    return uid, gid


def preflight(args):
    uid, gid = validate_preflight_host(args)
    checked_directory(ROOT / "pgsocket", uid, gid, 0o700, empty=True)
    checked_directory(ROOT / "pglog", uid, gid, 0o700)
    checked_directory(ROOT / "tablespaces", uid, gid, 0o700, empty=True)
    for name in ("pgbackrest-lock", "pgbackrest-spool"):
        checked_directory(ROOT / name, uid, gid, 0o700, empty=True)
    data = read_repository(ROOT, args.endpoint, args.bucket, args.repo_path)
    verify_stage_receipt(ROOT, stage_receipt(args, data, ROOT), 0, 0, gid)
    verify_data_tree(ROOT / "pgdata", uid, gid)


def add_preflight_arguments(parser):
    parser.add_argument("--source-machine-id", required=True)
    parser.add_argument("--target-machine-id", required=True)
    parser.add_argument("--pg-port", type=int, default=15432)
    parser.add_argument("--backup-label", required=True)
    parser.add_argument("--target-utc", required=True)
    parser.add_argument("--incident-utc", required=True)
    parser.add_argument("--timeline", type=int, required=True)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--bucket", required=True)
    parser.add_argument("--repo-path", required=True)
    parser.add_argument("--operator-confirm-quiescent-isolated-host", action="store_true")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_preflight_arguments(parser)
    args = parser.parse_args()
    try:
        preflight(args)
    except (Refused, OSError, KeyError, ValueError, UnicodeError) as error:
        reason = str(error) if isinstance(error, Refused) else "local preflight failed"
        print("TOR_RECOVERY_PG_PRESTART_REFUSED: " + reason, file=sys.stderr)
        return 2
    print("TOR_RECOVERY_PG_PRESTART_FILES_CHECKED "
          "configs_sanitized=false startup_authorized=false "
          "postgres_started=false wal_replayed=false recovery_accepted=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
