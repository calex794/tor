#!/usr/bin/env python3
"""AUTH-08c: invalidate sessions on an approved, promoted, offline recovery PG18.

Never starts or promotes PostgreSQL, starts TOR, or operates on a live host.
The default is a read-only count. A successful invalidation is not proof of
identity reconciliation or permission to publish the recovered service.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import subprocess
import sys

from tor_recovery_host import Refused, reject_live_markers, validate_target
from tor_recovery_pg import STAGE_RECEIPT, checked_directory, unique_fields
from tor_recovery_pg_prestart import ROOT, private_read
from tor_recovery_pg_run import REPLAY_RECEIPT, PSQL, as_postgres, check_runtime, invoke
from tor_recovery_pg_startup_config import stage_lock

DATABASE = re.compile(r"[a-z][a-z0-9_]{0,62}\Z")
SYSTEM_ID = re.compile(r"[1-9][0-9]{0,19}\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
ACTIVE = "revoked_at IS NULL AND expires_at>transaction_timestamp()"


def validate_selection(database, system_id):
    if (not DATABASE.fullmatch(database) or database in ("postgres", "template0", "template1")
            or not SYSTEM_ID.fullmatch(system_id) or int(system_id) >= 2**64):
        raise Refused("explicit application database and independent cluster identity required")


def operator_guard(database, system_id, port):
    validate_selection(database, system_id)
    if type(port) is not int or not 15432 <= port <= 15999:
        raise Refused("invalid isolated PostgreSQL port")
    # Every interpolated value is fixed or grammar-checked. Refuse the wrong
    # backend within the same transaction that would perform the mutation.
    return f"""
DO $guard$
BEGIN
    IF NOT (
        current_database() = '{database}'
        AND current_user = 'postgres' AND session_user = 'postgres'
        AND NOT pg_is_in_recovery()
        AND inet_server_addr() IS NULL
        AND current_setting('server_version_num')::integer BETWEEN 180000 AND 189999
        AND current_setting('data_directory') = '{ROOT}/pgdata'
        AND current_setting('unix_socket_directories') = '{ROOT}/pgsocket'
        AND current_setting('listen_addresses') = ''
        AND current_setting('port') = '{port}'
        AND current_setting('archive_mode') = 'off'
        AND current_setting('primary_conninfo') = ''
        AND (SELECT system_identifier::text FROM pg_control_system()) = '{system_id}'
        AND NOT EXISTS (
            SELECT 1 FROM pg_stat_activity
            WHERE backend_type = 'client backend' AND pid <> pg_backend_pid()
        )
    ) THEN
        RAISE EXCEPTION 'AUTH08C isolated promoted database guard failed';
    END IF;
END
$guard$;
"""


def invalidation_sql(guard=""):
    """One atomic operation, reused by the real disposable restoration drill."""
    return f"""
BEGIN READ WRITE;
SET LOCAL search_path=pg_catalog;
SET LOCAL lock_timeout='2s';
SET LOCAL statement_timeout='120s';
{guard}
LOCK TABLE public.sessions IN ACCESS EXCLUSIVE MODE;
{guard}
WITH ended AS (
    UPDATE public.sessions SET revoked_at=transaction_timestamp()
    WHERE {ACTIVE}
    RETURNING 1
) SELECT count(*) FROM ended;
DO $verify$
BEGIN
    IF EXISTS (SELECT 1 FROM public.sessions WHERE {ACTIVE}) THEN
        RAISE EXCEPTION 'AUTH08C live sessions remain';
    END IF;
END
$verify$;
COMMIT;
"""


def plan_sql(guard):
    return f"""
BEGIN READ ONLY;
SET LOCAL search_path=pg_catalog;
SET LOCAL statement_timeout='30s';
{guard}
SELECT count(*) FROM public.sessions WHERE {ACTIVE};
COMMIT;
"""


def verify_receipts(root, args):
    evidence = root / "evidence"
    records = []
    for name, pin in ((STAGE_RECEIPT, args.stage_sha256),
                      (REPLAY_RECEIPT, args.replay_sha256)):
        if not SHA256.fullmatch(pin) or pin == "0" * 64:
            raise Refused("independently pinned recovery receipt digest required")
        raw = private_read(evidence / name, 0, 0, (0o600,), 16384)
        if hashlib.sha256(raw).hexdigest() != pin:
            raise Refused("recovery receipt does not match independent pin")
        records.append(json.loads(raw, object_pairs_hook=unique_fields))
    stage, replay = records
    if (stage.get("scope") != "physical_files_staged_only"
            or stage.get("source_machine_id") != args.source_machine_id
            or stage.get("target_machine_id") != args.target_machine_id
            or stage.get("pg_port") != args.pg_port
            or replay.get("scope") != "paused_replay_observed_then_stopped_only"
            or replay.get("stage_receipt_sha256") != args.stage_sha256
            or replay.get("pg_port") != args.pg_port
            or any(replay.get(key) != stage.get(key)
                   for key in ("backup_label", "target_utc", "timeline"))
            or replay.get("stable_paused_replay_observed") is not True
            or replay.get("postgres_shutdown_confirmed") is not True
            or replay.get("promotion_authorized") is not False
            or replay.get("recovery_accepted") is not False):
        raise Refused("inconsistent staged and paused recovery evidence")


def execute(args):
    if os.geteuid() != 0 or "GITHUB_ACTIONS" in os.environ:
        raise Refused("root on independent recovery host required; CI prohibited")
    if not args.operator_confirm_promoted_offline_recovery:
        raise Refused("separate review of promoted isolated recovery required")
    validate_selection(args.database, args.system_id)
    machine = Path("/etc/machine-id").read_text(encoding="ascii").strip()
    validate_target(args.source_machine_id, args.target_machine_id, machine, args.pg_port)
    reject_live_markers()
    postgres = pwd.getpwnam("postgres")
    uid, gid = postgres.pw_uid, postgres.pw_gid
    if uid == 0 or gid == 0:
        raise Refused("unprivileged postgres OS identity required")
    checked_directory(ROOT, 0, gid, 0o710)
    checked_directory(ROOT / "evidence", 0, 0, 0o700)
    checked_directory(ROOT / "pgdata", uid, gid, 0o700)
    checked_directory(ROOT / "pgsocket", uid, gid, 0o700)
    with stage_lock(ROOT):
        verify_receipts(ROOT, args)
        check_runtime(ROOT)
        guard = operator_guard(args.database, args.system_id, args.pg_port)
        statement = invalidation_sql(guard) if args.apply else plan_sql(guard)
        command = as_postgres(
            PSQL, "-X", "-w", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1",
            "-h", str(ROOT / "pgsocket"), "-p", str(args.pg_port),
            "-U", "postgres", "-d", args.database, "-c", statement, query=True,
        )
        result = invoke(command, 145, capture=True)
        output = result.stdout.strip()
        if result.returncode != 0 or not re.fullmatch(r"0|[1-9][0-9]{0,18}", output):
            raise Refused("database guard or operation failed; keep TOR offline")
        return int(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-machine-id", required=True)
    parser.add_argument("--target-machine-id", required=True)
    parser.add_argument("--pg-port", type=int, default=15432)
    parser.add_argument("--database", required=True)
    parser.add_argument("--system-id", required=True)
    parser.add_argument("--stage-sha256", required=True)
    parser.add_argument("--replay-sha256", required=True)
    parser.add_argument("--operator-confirm-promoted-offline-recovery", action="store_true")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    try:
        count = execute(args)
    except (Refused, OSError, KeyError, ValueError, AttributeError, subprocess.SubprocessError):
        print("TOR_RECOVERY_AUTH_REFUSED: keep TOR offline; inspect privately", file=sys.stderr)
        return 2
    status = "SESSIONS_INVALIDATED" if args.apply else "PLAN_ONLY"
    print(f"TOR_RECOVERY_AUTH_{status} sessions={count} recovery_accepted=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
