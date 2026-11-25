#!/usr/bin/env python3
"""RECOVERY-08: stage fresh, pinned PostgreSQL recovery configuration only.

Requires an approved quiescent independent host and a successful RECOVERY-07
preflight. Never starts PostgreSQL, retrieves WAL, runs SQL or promotes.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys

from tor_recovery_host import Refused
from tor_recovery_pg import STAGE_RECEIPT, checkpoint
from tor_recovery_pg_prestart import (
    ROOT, MAX_CONFIG, add_preflight_arguments, preflight, private_read,
    validate_preflight_host,
)

STARTUP_DIR = "pg-startup"
ATTEMPT = "pg-config-attempt"
ORIGINAL_AUTO = "pg-original-auto.conf"
CONFIG_RECEIPT = "pg-config-receipt.json"
TEMP_AUTO = ".tor-recovery-empty-auto.conf"
RESOURCE_LIMITS = {
    "max_connections": (10, 1000),
    "max_prepared_transactions": (0, 1024),
    "max_locks_per_transaction": (10, 4096),
    "max_wal_senders": (0, 128),
    "max_worker_processes": (0, 128),
    "shared_buffers_mb": (16, 4096),
}
SAFE_ROOT = re.compile(r"/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*\Z")


def sha(data):
    return hashlib.sha256(data).hexdigest()


def resource_settings(args):
    values = {}
    for name, (minimum, maximum) in RESOURCE_LIMITS.items():
        value = getattr(args, name, None)
        if type(value) is not int or not minimum <= value <= maximum:
            raise Refused("explicit supported PostgreSQL resource settings required")
        values[name] = value
    if values["max_wal_senders"] >= values["max_connections"]:
        raise Refused("max_wal_senders must be smaller than max_connections")
    return values


def render_startup(root, args):
    # Only the fixed workspace is used by the CLI. Validate path syntax even
    # in local fixture helpers, before placing paths in config/shell strings.
    if (not SAFE_ROOT.fullmatch(str(root))
            or any(part in (".", "..") for part in root.parts)):
        raise Refused("unsafe recovery configuration path")
    if type(args.pg_port) is not int or not 15432 <= args.pg_port <= 15999:
        raise Refused("isolated PostgreSQL port required")
    target = checkpoint(args.backup_label, args.target_utc, args.incident_utc,
                        args.timeline)
    values = resource_settings(args)
    startup = root / "config" / STARTUP_DIR
    archive_get = (
        "/usr/bin/env -i PATH=/usr/bin:/bin LC_ALL=C /usr/bin/pgbackrest "
        f"--config={root}/config/pgbackrest-recovery.conf "
        f"--config-include-path={root}/config/pgbackrest-empty "
        f"--stanza=tor --repo=1 --pg1-path={root}/pgdata "
        'archive-get "%f" "%p"'
    )
    config = f"""# Generated for an isolated recovery review, not a live server.
data_directory = '{root}/pgdata'
hba_file = '{startup}/pg_hba.conf'
ident_file = '{startup}/pg_ident.conf'
external_pid_file = ''
listen_addresses = ''
port = {args.pg_port}
unix_socket_directories = '{root}/pgsocket'
unix_socket_permissions = 0700
ssl = off
archive_mode = off
archive_command = ''
archive_library = ''
primary_conninfo = ''
primary_slot_name = ''
shared_preload_libraries = ''
session_preload_libraries = ''
local_preload_libraries = ''
archive_cleanup_command = ''
recovery_end_command = ''
hot_standby = on
restore_command = '{archive_get}'
recovery_target_time = '{target}'
recovery_target_timeline = '{args.timeline}'
recovery_target_inclusive = off
recovery_target_action = 'pause'
max_connections = {values['max_connections']}
max_prepared_transactions = {values['max_prepared_transactions']}
max_locks_per_transaction = {values['max_locks_per_transaction']}
max_wal_senders = {values['max_wal_senders']}
max_worker_processes = {values['max_worker_processes']}
shared_buffers = '{values['shared_buffers_mb']}MB'
"""
    hba = (
        "local all postgres peer\n"
        "local all all reject\n"
        "local replication all reject\n"
        "host all all 0.0.0.0/0 reject\n"
        "host all all ::/0 reject\n"
        "host replication all 0.0.0.0/0 reject\n"
        "host replication all ::/0 reject\n"
    )
    return {
        "postgresql.conf": config.encode("ascii"),
        "pg_hba.conf": hba.encode("ascii"),
        "pg_ident.conf": b"# No inherited role mappings.\n",
    }


@contextmanager
def stage_lock(root, uid=0, gid=0):
    # The stage created this lock. Do not create a fresh lock to bypass an
    # incomplete stage, or follow a symlink or block opening a special file.
    fd = os.open(root / "evidence" / "pg-restore.lock",
                 os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
                or (info.st_uid, info.st_gid) != (uid, gid)
                or stat.S_IMODE(info.st_mode) != 0o600):
            raise Refused("unsafe stage lock")
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(fd)


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_exclusive(path, content, uid, gid, mode):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "wb") as handle:
        os.fchown(handle.fileno(), uid, gid)
        os.fchmod(handle.fileno(), mode)
        handle.write(content)
        handle.flush()
        os.fsync(handle.fileno())


def reject_prior_attempt(root):
    for path in (
        root / "config" / STARTUP_DIR,
        root / "evidence" / ATTEMPT,
        root / "evidence" / ORIGINAL_AUTO,
        root / "evidence" / CONFIG_RECEIPT,
        root / "pgdata" / TEMP_AUTO,
    ):
        if os.path.lexists(path):
            raise Refused("prior config preparation exists; inspect separately")


def config_receipt_bytes(args, stage, original, files):
    receipt = {
        "schema_version": 1,
        "scope": "trusted_startup_config_staged_only",
        "stage_receipt_sha256": sha(stage),
        "original_auto_conf_sha256": sha(original),
        "empty_auto_conf_sha256": sha(b""),
        "config_sha256": {name: sha(content) for name, content in files.items()},
        "resource_settings": resource_settings(args),
        "startup_authorized": False,
    }
    encoded = (json.dumps(receipt, sort_keys=True) + "\n").encode("ascii")
    return encoded


def prepare_files(root, args, pg_uid, pg_gid, operator_uid=0, operator_gid=0):
    # Caller MUST have verified the host and full preflight under stage_lock.
    files = render_startup(root, args)
    reject_prior_attempt(root)
    evidence = root / "evidence"
    config = root / "config"
    pgdata = root / "pgdata"
    original = private_read(pgdata / "postgresql.auto.conf",
                            pg_uid, pg_gid, (0o400, 0o600), MAX_CONFIG)
    stage = private_read(evidence / STAGE_RECEIPT,
                         operator_uid, operator_gid, (0o600,), 16384)
    # Every partial attempt is preserved. No deletion, reset or retry.
    write_exclusive(evidence / ATTEMPT, b"RECOVERY-08 attempted\n",
                    operator_uid, operator_gid, 0o600)
    sync_directory(evidence)
    startup = config / STARTUP_DIR
    startup.mkdir(mode=0o700)
    os.chown(startup, operator_uid, pg_gid)
    startup.chmod(0o750)
    for name, content in files.items():
        write_exclusive(startup / name, content, operator_uid, pg_gid, 0o640)
    sync_directory(startup)
    sync_directory(config)
    write_exclusive(evidence / ORIGINAL_AUTO, original,
                    operator_uid, operator_gid, 0o600)
    sync_directory(evidence)
    # Only now replace this specific inherited config with a new empty file.
    # Never import its settings or print its possibly sensitive content.
    temporary = pgdata / TEMP_AUTO
    write_exclusive(temporary, b"", pg_uid, pg_gid, 0o600)
    os.replace(temporary, pgdata / "postgresql.auto.conf")
    sync_directory(pgdata)
    encoded = config_receipt_bytes(args, stage, original, files)
    write_exclusive(evidence / CONFIG_RECEIPT, encoded,
                    operator_uid, operator_gid, 0o600)
    sync_directory(evidence)


def execute(args):
    uid, gid = validate_preflight_host(args)
    with stage_lock(ROOT):
        preflight(args)
        reject_prior_attempt(ROOT)
        render_startup(ROOT, args)
        if args.prepare_config:
            prepare_files(ROOT, args, uid, gid)


def add_resource_arguments(parser):
    for name in RESOURCE_LIMITS:
        parser.add_argument("--" + name.replace("_", "-"), required=True, type=int)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_preflight_arguments(parser)
    add_resource_arguments(parser)
    parser.add_argument("--prepare-config", action="store_true")
    args = parser.parse_args()
    try:
        execute(args)
    except (Refused, OSError, KeyError, ValueError, UnicodeError) as error:
        reason = str(error) if isinstance(error, Refused) else "local config preparation failed"
        print("TOR_RECOVERY_PG_CONFIG_REFUSED: " + reason, file=sys.stderr)
        return 2
    status = "FILES_PREPARED" if args.prepare_config else "PLAN_ONLY"
    print(f"TOR_RECOVERY_PG_CONFIG_{status} "
          "startup_authorized=false postgres_started=false "
          "wal_replayed=false recovery_accepted=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
