#!/usr/bin/env python3
"""RECOVERY-09c: bounded, operator-authorized paused replay and shutdown.

Default is read-only verification. Never promotes, resumes WAL or starts
TOR. A successful witness is not independent recovery acceptance.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import sys
import time

from tor_recovery_host import PG_BIN, Refused
from tor_recovery_pg import STAGE_RECEIPT, checkpoint, utc_time, checked_directory
from tor_recovery_pg_prestart import (
    ROOT, add_preflight_arguments, preflight, private_read, validate_preflight_host,
)
from tor_recovery_pg_startup_config import (
    CONFIG_RECEIPT, STARTUP_DIR, add_resource_arguments, render_startup, stage_lock,
    sha, sync_directory, write_exclusive,
)
from tor_recovery_pg_config_verify import verify_prepared_files

RUNUSER = Path("/usr/sbin/runuser")
ENV = Path("/usr/bin/env")
PGBACKREST = Path("/usr/bin/pgbackrest")
PG_CTL = PG_BIN / "pg_ctl"
POSTGRES = PG_BIN / "postgres"
PSQL = PG_BIN / "psql"
LAUNCH_ATTEMPT = "pg-launch-attempt"
REPLAY_RECEIPT = "pg-paused-replay-receipt.json"
LOG = "pg-paused-replay.log"
CLEAN_ENV = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "TZ": "UTC"}
REPLAY_LSN = re.compile(r"[0-9A-F]{1,8}/[0-9A-F]{1,8}\Z")
REPLAY_TIME = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{6}Z\Z")


def trusted_directory(path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
        raise Refused("untrusted recovery runtime directory")


def check_runtime(root):
    if root.resolve(strict=True) != root:
        raise Refused("recovery workspace must be canonical")
    for parent in root.parents:
        trusted_directory(parent)
    for path in (RUNUSER, ENV, PGBACKREST, PG_CTL, POSTGRES, PSQL):
        for parent in path.parents:
            trusted_directory(parent)
        info = path.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != 0
                or info.st_mode & 0o022 or not info.st_mode & 0o111):
            raise Refused("trusted installed recovery executable required")


def timeout_seconds(args):
    value = args.replay_timeout_seconds
    if type(value) is not int or not 60 <= value <= 14400:
        raise Refused("explicit replay timeout must be 60-14400 seconds")
    return value


def as_postgres(executable, *arguments, query=False):
    clean = [str(RUNUSER), "-u", "postgres", "--", str(ENV), "-i",
             "PATH=/usr/bin:/bin", "LC_ALL=C", "TZ=UTC", "HOME=/nonexistent"]
    if query:
        clean.extend([
            "PGCONNECT_TIMEOUT=3",
            "PGOPTIONS=-c search_path=pg_catalog -c statement_timeout=2000 "
            "-c default_transaction_read_only=on -c session_preload_libraries= "
            "-c local_preload_libraries=",
        ])
    return [*clean, str(executable), *arguments]


def replay_sql(root, args):
    # render_startup checks safe path, numeric port, target and resource bounds.
    render_startup(root, args)
    target = checkpoint(args.backup_label, args.target_utc,
                        args.incident_utc, args.timeline)
    startup = root / "config" / STARTUP_DIR
    # All interpolated values are fixed paths or validated numbers/timestamps.
    # Use catalog functions and no application objects, identifiers or SQL input.
    return f"""SELECT CASE WHEN pg_catalog.pg_is_in_recovery() THEN
      pg_catalog.json_build_array(
        true, pg_catalog.pg_get_wal_replay_pause_state(),
        pg_catalog.current_setting('server_version_num')::integer BETWEEN 180000 AND 189999
        AND pg_catalog.current_setting('config_file') = '{startup}/postgresql.conf'
        AND pg_catalog.current_setting('data_directory') = '{root}/pgdata'
        AND pg_catalog.current_setting('hba_file') = '{startup}/pg_hba.conf'
        AND pg_catalog.current_setting('ident_file') = '{startup}/pg_ident.conf'
        AND pg_catalog.current_setting('listen_addresses') = ''
        AND pg_catalog.current_setting('unix_socket_directories') = '{root}/pgsocket'
        AND pg_catalog.current_setting('port') = '{args.pg_port}'
        AND pg_catalog.current_setting('ssl') = 'off'
        AND pg_catalog.current_setting('archive_mode') = 'off'
        AND pg_catalog.current_setting('primary_conninfo') = ''
        AND pg_catalog.current_setting('hot_standby') = 'on'
        AND pg_catalog.current_setting('transaction_read_only') = 'on'
        AND pg_catalog.current_setting('recovery_target_time')::timestamptz = '{target}'::timestamptz
        AND pg_catalog.current_setting('recovery_target_timeline') = '{args.timeline}'
        AND pg_catalog.current_setting('recovery_target_inclusive') = 'off'
        AND pg_catalog.current_setting('recovery_target_action') = 'pause',
        pg_catalog.pg_last_wal_replay_lsn()::text,
        pg_catalog.to_char(pg_catalog.pg_last_xact_replay_timestamp() AT TIME ZONE 'UTC',
                          'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
      ) ELSE '[false,"primary",false,null,null]'::json END"""


def commands(root, args):
    wait = timeout_seconds(args)
    sql = replay_sql(root, args)
    pgdata = str(root / "pgdata")
    start = as_postgres(
        PG_CTL, "-D", pgdata, "-p", str(POSTGRES),
        "-l", str(root / "pglog" / LOG), "-w", "-t", str(wait),
        "-o", f"-c config_file={root}/config/{STARTUP_DIR}/postgresql.conf", "start",
    )
    query = as_postgres(
        PSQL, "-X", "-w", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1",
        "-h", str(root / "pgsocket"), "-p", str(args.pg_port),
        "-U", "postgres", "-d", "postgres", "-c", sql, query=True,
    )
    stop = as_postgres(PG_CTL, "-D", pgdata, "-m", "fast",
                       "-w", "-t", "60", "stop")
    return start, query, stop


def invoke(command, timeout, capture=False):
    return subprocess.run(
        command, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
        stderr=subprocess.DEVNULL, env=CLEAN_ENV, cwd="/", text=True,
        timeout=timeout, check=False,
    )


def parse_witness(raw, target_utc):
    if not isinstance(raw, str) or len(raw) > 1024:
        raise Refused("invalid PostgreSQL replay witness")
    try:
        row = json.loads(raw)
    except (ValueError, UnicodeError) as error:
        raise Refused("invalid PostgreSQL replay witness") from error
    if not isinstance(row, list) or len(row) != 5:
        raise Refused("unexpected PostgreSQL replay witness")
    recovery, pause, config, lsn, stamp = row
    if recovery is not True or config is not True:
        raise Refused("unexpected primary or effective PostgreSQL configuration")
    if pause in ("not paused", "pause requested"):
        return None
    if pause != "paused":
        raise Refused("unknown WAL replay state")
    if (not isinstance(lsn, str) or not REPLAY_LSN.fullmatch(lsn) or lsn == "0/0"
            or not isinstance(stamp, str) or not REPLAY_TIME.fullmatch(stamp)):
        raise Refused("paused replay lacks a valid WAL/time witness")
    try:
        replay_time = datetime.strptime(stamp, "%Y-%m-%dT%H:%M:%S.%fZ").replace(
            tzinfo=timezone.utc)
    except ValueError as error:
        raise Refused("invalid last replay transaction time") from error
    if replay_time >= utc_time(target_utc):
        raise Refused("last replayed transaction is not before exclusive target")
    return {"replay_lsn": lsn, "last_replayed_transaction_utc": stamp}


def wait_paused(query, target_utc, deadline):
    previous = None
    while time.monotonic() < deadline:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        result = invoke(query, min(remaining, 6), capture=True)
        witness = parse_witness(result.stdout, target_utc) if result.returncode == 0 else None
        if witness is not None and witness == previous:
            return witness
        previous = witness
        time.sleep(0.25)
    raise Refused("actual stable paused WAL replay not witnessed before deadline")


def reject_launch_artifacts(root):
    for path in (root / "evidence" / LAUNCH_ATTEMPT,
                 root / "evidence" / REPLAY_RECEIPT, root / "pglog" / LOG):
        if os.path.lexists(path):
            raise Refused("previous launch attempt exists; manual investigation required")


def run_paused(root, args, pg_uid, pg_gid, operator_uid=0, operator_gid=0):
    # Caller has verified operational host, isolation confirmation, runtime
    # and exact prepared configuration under the still-held stage lock.
    start, query, stop = commands(root, args)
    reject_launch_artifacts(root)
    evidence = root / "evidence"
    # Bind the observation to the exact private verified stage/config evidence.
    receipts = {
        name: private_read(evidence / name, operator_uid, operator_gid, (0o600,), 16384)
        for name in (STAGE_RECEIPT, CONFIG_RECEIPT)
    }
    attempt = evidence / LAUNCH_ATTEMPT
    write_exclusive(attempt, b"RECOVERY-09c attempted\n",
                    operator_uid, operator_gid, 0o600)
    sync_directory(evidence)
    write_exclusive(root / "pglog" / LOG, b"", pg_uid, pg_gid, 0o600)
    sync_directory(root / "pglog")
    wait = timeout_seconds(args)
    deadline = time.monotonic() + wait
    # pg_ctl can return before recovery reaches the target, and on timeout
    # the postmaster may still be running. Always attempt bounded shutdown.
    try:
        if invoke(start, wait + 10).returncode != 0:
            raise Refused("PostgreSQL start failed or timed out")
        witness = wait_paused(query, args.target_utc, deadline)
    finally:
        try:
            stopped = invoke(stop, 70)
            if stopped.returncode != 0 or os.path.lexists(root / "pgdata/postmaster.pid"):
                raise Refused("shutdown not confirmed")
            checked_directory(root / "pgsocket", pg_uid, pg_gid, 0o700, empty=True)
        except (Refused, OSError, subprocess.SubprocessError) as error:
            raise Refused("shutdown not confirmed; keep host isolated for manual inspection") from error
    # Never promote or remove the recovery signal. The operator must
    # independently review transaction/business witnesses before acceptance.
    private_read(root / "pgdata/recovery.signal", pg_uid, pg_gid, (0o400, 0o600), 0)
    for name, original in receipts.items():
        if private_read(evidence / name, operator_uid, operator_gid, (0o600,), 16384) != original:
            raise Refused("verified preparation evidence changed during replay")
    report = {
        "schema_version": 1, "scope": "paused_replay_observed_then_stopped_only",
        "backup_label": args.backup_label, "target_utc": args.target_utc,
        "timeline": args.timeline, "pg_port": args.pg_port,
        "stage_receipt_sha256": sha(receipts[STAGE_RECEIPT]),
        "config_receipt_sha256": sha(receipts[CONFIG_RECEIPT]),
        "stable_paused_replay_observed": True,
        "postgres_shutdown_confirmed": True, "recovery_signal_preserved": True,
        **witness,
        "exact_target_transaction_boundary_verified": False,
        "promotion_authorized": False, "recovery_accepted": False,
    }
    write_exclusive(evidence / REPLAY_RECEIPT,
                    (json.dumps(report, sort_keys=True) + "\n").encode("ascii"),
                    operator_uid, operator_gid, 0o600)
    sync_directory(evidence)


def execute(args):
    timeout_seconds(args)
    uid, gid = validate_preflight_host(args)
    with stage_lock(ROOT):
        preflight(args)
        verify_prepared_files(ROOT, args, uid, gid)
        reject_launch_artifacts(ROOT)
        if not args.run_paused_replay:
            return
        if not args.operator_confirm_reviewed_isolated_execution:
            raise Refused("separate operator confirmation of reviewed isolated execution required")
        check_runtime(ROOT)
        run_paused(ROOT, args, uid, gid)


def interrupted(_signum, _frame):
    raise InterruptedError("operator interrupted recovery")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_preflight_arguments(parser)
    add_resource_arguments(parser)
    parser.add_argument("--replay-timeout-seconds", type=int, default=300)
    parser.add_argument("--run-paused-replay", action="store_true")
    parser.add_argument("--operator-confirm-reviewed-isolated-execution", action="store_true")
    args = parser.parse_args()
    prior = signal.signal(signal.SIGTERM, interrupted)
    try:
        execute(args)
    except (Refused, OSError, KeyError, ValueError, UnicodeError,
            subprocess.SubprocessError, KeyboardInterrupt) as error:
        reason = str(error) if isinstance(error, Refused) else "local replay attempt failed"
        print("TOR_RECOVERY_PG_REPLAY_REFUSED: " + reason, file=sys.stderr)
        return 2
    finally:
        signal.signal(signal.SIGTERM, prior)
    status = "PAUSED_REPLAY_OBSERVED_AND_STOPPED" if args.run_paused_replay else "PLAN_ONLY"
    print("TOR_RECOVERY_PG_" + status
          + " exact_target_transaction_boundary_verified=false "
            "promotion_authorized=false recovery_accepted=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
