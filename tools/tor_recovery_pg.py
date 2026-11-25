#!/usr/bin/env python3
"""RECOVERY-03a: stage a selected PG18 backup for exact-point WAL replay.

Read-only plan by default. Explicit --restore runs pgBackRest as postgres on
the prepared independent host; never starts PostgreSQL, promotes or deploys.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import stat
import subprocess
import sys

from tor_recovery_host import (
    NAME, PARENT, Refused, reject_live_markers, validate_target,
)

ROOT = PARENT / NAME
BACKUP_LABEL = re.compile(r"[0-9]{8}-[0-9]{6}F(?:_[0-9]{8}-[0-9]{6}[DI])?\Z")
UTC = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z\Z")
ENDPOINT = re.compile(r"[a-z0-9](?:[a-z0-9.-]{1,251}[a-z0-9])?\Z")
BUCKET = re.compile(r"[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]\Z")
REGION = re.compile(r"[a-z0-9-]{2,64}\Z")
REPO_PATH = re.compile(r"/[a-zA-Z0-9_-]+(?:/[a-zA-Z0-9_-]+)*\Z")
SECRET = re.compile(r"[A-Za-z0-9+/=_!@.%:-]{8,512}\Z")
STAGE_RECEIPT = "pg-stage-receipt.json"
FIELDS = {
    "endpoint", "bucket", "region", "repo_path", "access_key",
    "secret_key", "cipher_pass",
}


def utc_time(value):
    if not isinstance(value, str) or not UTC.fullmatch(value):
        raise Refused("explicit whole-second UTC timestamp required")
    try:
        return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as error:
        raise Refused("invalid UTC timestamp") from error


def checkpoint(label, target, incident, timeline):
    if not isinstance(label, str) or not BACKUP_LABEL.fullmatch(label):
        raise Refused("explicit pgBackRest backup label required; latest forbidden")
    if type(timeline) is not int or not 1 <= timeline <= 4294967295:
        raise Refused("explicit positive recovery timeline required")
    when = utc_time(target)
    if when >= utc_time(incident):
        raise Refused("recovery target must precede the incident cutoff")
    if utc_time(incident) > datetime.now(timezone.utc):
        raise Refused("incident cutoff cannot be in the future")
    return when.strftime("%Y-%m-%d %H:%M:%S+00")


def unique_fields(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Refused("duplicate repository field")
        result[key] = value
    return result


def repository_settings(data, endpoint, bucket, repo_path):
    if not isinstance(data, dict) or set(data) != FIELDS:
        raise Refused("unexpected or missing recovery repository settings")
    for field, pattern in (
        ("endpoint", ENDPOINT), ("bucket", BUCKET), ("region", REGION),
        ("repo_path", REPO_PATH), ("access_key", SECRET),
        ("secret_key", SECRET), ("cipher_pass", SECRET),
    ):
        if not isinstance(data[field], str) or not pattern.fullmatch(data[field]):
            raise Refused("invalid recovery repository field")
    if (".." in data["endpoint"] or "." not in data["endpoint"]
            or ".." in data["bucket"] or data["bucket"] == "tore"):
        raise Refused("independent backup repository required, not live bucket")
    if (data["endpoint"], data["bucket"], data["repo_path"]) != (
            endpoint, bucket, repo_path):
        raise Refused("repository differs from independent operator selection")
    if len(data["cipher_pass"]) < 32:
        raise Refused("recovery cipher passphrase too short")
    return data


def checked_directory(path, uid, gid, mode, empty=False):
    info = path.lstat()
    if (not stat.S_ISDIR(info.st_mode) or path.resolve(strict=True) != path
            or (info.st_uid, info.st_gid) != (uid, gid)
            or stat.S_IMODE(info.st_mode) != mode):
        raise Refused("recovery workspace directory missing or unsafe")
    if empty and any(path.iterdir()):
        raise Refused("restore destination is not empty; never overwrite")


def read_repository(root, endpoint, bucket, repo_path):
    path = root / "evidence" / "pgbackrest-repository.json"
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as handle:
        info = os.fstat(handle.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != 0
                or stat.S_IMODE(info.st_mode) != 0o600 or info.st_nlink != 1
                or info.st_size > 16384):
            raise Refused("private repository settings file unsafe")
        raw = handle.read(16385)
    if len(raw) > 16384:
        raise Refused("repository settings too large")
    try:
        data = json.loads(raw, object_pairs_hook=unique_fields)
    except (ValueError, UnicodeError) as error:
        raise Refused("invalid private repository settings") from error
    return repository_settings(data, endpoint, bucket, repo_path)


def render_config(data, root, port):
    # Generated from a fixed allowlist, not imported source-host configuration.
    return f"""[global]
repo1-type=s3
repo1-s3-endpoint={data['endpoint']}
repo1-s3-bucket={data['bucket']}
repo1-s3-region={data['region']}
repo1-s3-uri-style=path
repo1-s3-key-type=shared
repo1-s3-key={data['access_key']}
repo1-s3-key-secret={data['secret_key']}
repo1-path={data['repo_path']}
repo1-cipher-type=aes-256-cbc
repo1-cipher-pass={data['cipher_pass']}
repo1-storage-verify-tls=y
log-level-console=off
log-level-file=off
log-level-stderr=off
log-path={root}/pglog
lock-path={root}/pgbackrest-lock
spool-path={root}/pgbackrest-spool
process-max=2
[tor]
pg1-path={root}/pgdata
pg1-port={port}
pg1-socket-path={root}/pgsocket
"""


def stage_receipt(args, data, root):
    # Private operator evidence, not a signature or a DR acceptance report.
    return {
        "schema_version": 1,
        "scope": "physical_files_staged_only",
        "source_machine_id": args.source_machine_id,
        "target_machine_id": args.target_machine_id,
        "pg_port": args.pg_port,
        "backup_label": args.backup_label,
        "target_utc": args.target_utc,
        "incident_utc": args.incident_utc,
        "timeline": args.timeline,
        "repository_config_sha256": hashlib.sha256(
            render_config(data, root, args.pg_port).encode("ascii")
        ).hexdigest(),
    }


def write_stage_receipt(root, receipt):
    path = root / "evidence" / STAGE_RECEIPT
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w", encoding="ascii") as handle:
        os.fchmod(handle.fileno(), 0o600)
        json.dump(receipt, handle, sort_keys=True)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    # A failure leaves a partial attempt; the operator must inspect it.


def restore_command(root, label, target, timeline):
    # No --force, --delta, --link-all, --db-include, or automatic set fallback.
    # All original tablespaces must remain inside the isolated restore workspace.
    return [
        "/usr/sbin/runuser", "-u", "postgres", "--",
        "/usr/bin/env", "-i", "PATH=/usr/bin:/bin", "LC_ALL=C",
        "/usr/bin/pgbackrest",
        f"--config={root}/config/pgbackrest-recovery.conf",
        f"--config-include-path={root}/config/pgbackrest-empty",
        "--stanza=tor", "--repo=1", f"--pg1-path={root}/pgdata",
        f"--tablespace-map-all={root}/tablespaces",
        f"--set={label}", "--type=time", f"--target={target}",
        f"--target-timeline={timeline}", "--target-action=shutdown",
        "--target-exclusive", "restore",
    ]


def prepare_restore_config(root, data, pg_uid, pg_gid, port):
    # Existing/partial recovery attempts are never overwritten automatically.
    for path in ("tablespaces", "pgbackrest-lock", "pgbackrest-spool"):
        directory = root / path
        directory.mkdir(mode=0o700)
        os.chown(directory, pg_uid, pg_gid)
        directory.chmod(0o700)
    empty = root / "config" / "pgbackrest-empty"
    empty.mkdir(mode=0o750)
    os.chown(empty, 0, pg_gid)
    empty.chmod(0o750)
    path = root / "config" / "pgbackrest-recovery.conf"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w", encoding="ascii") as handle:
        handle.write(render_config(data, root, port))
        handle.flush()
        os.fchown(handle.fileno(), 0, pg_gid)
        os.fchmod(handle.fileno(), 0o640)
        os.fsync(handle.fileno())


def check_workspace(root, postgres):
    uid, gid = postgres.pw_uid, postgres.pw_gid
    checked_directory(root, 0, gid, 0o710)
    checked_directory(root / "config", 0, gid, 0o750)
    checked_directory(root / "evidence", 0, 0, 0o700)
    checked_directory(root / "pgdata", uid, gid, 0o700, empty=True)
    checked_directory(root / "pgsocket", uid, gid, 0o700, empty=True)
    checked_directory(root / "pglog", uid, gid, 0o700)
    for name in ("tablespaces", "pgbackrest-lock", "pgbackrest-spool",
                 "config/pgbackrest-empty", "config/pgbackrest-recovery.conf",
                 "evidence/" + STAGE_RECEIPT):
        if os.path.lexists(root / name):
            raise Refused("previous restore attempt exists; inspect separately")


def stage_backup(args, target):
    if os.geteuid() != 0 or "GITHUB_ACTIONS" in os.environ:
        raise Refused("operator-only independent restore host required; CI prohibited")
    machine = Path("/etc/machine-id").read_text(encoding="ascii").strip()
    validate_target(args.source_machine_id, args.target_machine_id, machine, args.pg_port)
    reject_live_markers()
    postgres = pwd.getpwnam("postgres")
    if postgres.pw_uid == 0 or postgres.pw_gid == 0:
        raise Refused("unprivileged postgres account required")
    checked_directory(ROOT, 0, postgres.pw_gid, 0o710)
    checked_directory(ROOT / "evidence", 0, 0, 0o700)
    # Exclusive nonblocking lock, in the private operator directory.
    lock = os.open(ROOT / "evidence" / "pg-restore.lock",
                   os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(lock)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != 0
                or stat.S_IMODE(info.st_mode) != 0o600 or info.st_nlink != 1):
            raise Refused("unsafe recovery lock")
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        check_workspace(ROOT, postgres)
        data = read_repository(ROOT, args.endpoint, args.bucket, args.repo_path)
        if args.restore:
            if not args.operator_confirm_read_only_repository:
                raise Refused("independent read-only IAM review confirmation required")
            prepare_restore_config(ROOT, data, postgres.pw_uid, postgres.pw_gid,
                                   args.pg_port)
            result = subprocess.run(
                restore_command(ROOT, args.backup_label, target, args.timeline),
                check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                env={"PATH": "/usr/sbin:/usr/bin:/bin", "LC_ALL": "C"},
            )
            if result.returncode != 0:
                raise Refused("pgBackRest restore failed; partial workspace retained, do not start")
            pgdata = ROOT / "pgdata"
            if ((pgdata / "PG_VERSION").is_symlink()
                    or (pgdata / "PG_VERSION").read_text(encoding="ascii").strip() != "18"
                    or not (pgdata / "global" / "pg_control").is_file()
                    or not (pgdata / "recovery.signal").is_file()):
                raise Refused("restored PG18 files or recovery signal missing")
            write_stage_receipt(ROOT, stage_receipt(args, data, ROOT))
        # Never run postgres or pg_ctl here; restored settings require review.
    finally:
        os.close(lock)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
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
    parser.add_argument("--restore", action="store_true")
    parser.add_argument("--operator-confirm-read-only-repository", action="store_true")
    args = parser.parse_args()
    try:
        target = checkpoint(args.backup_label, args.target_utc, args.incident_utc,
                            args.timeline)
        if args.restore:
            stage_backup(args, target)
    except (Refused, OSError, KeyError, ValueError, subprocess.SubprocessError) as error:
        reason = str(error) if isinstance(error, Refused) else "local recovery operation failed"
        print("TOR_RECOVERY_PG_REFUSED: " + reason, file=sys.stderr)
        return 2
    status = "FILES_STAGED" if args.restore else "PLAN_ONLY"
    print(f"TOR_RECOVERY_PG_{status} wal_replayed=false "
          "postgres_started=false recovery_accepted=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
