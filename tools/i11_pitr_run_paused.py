#!/usr/bin/env python3
"""Real start/query/stop fixture for RECOVERY-09c; no operator entrypoint.

The existing network-free, unprivileged container already runs as the
postgres OS account. Adapt only the runuser prefix; all PostgreSQL
commands, witnesses, shutdown and receipt creation remain real.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import pwd
import sys
from unittest.mock import patch

from i11_pitr_prepare_config import prepare
from tor_recovery_host import Refused
from tor_recovery_pg import STAGE_RECEIPT, unique_fields
from tor_recovery_pg_prestart import private_read, verify_data_tree, verify_stage_receipt
from tor_recovery_pg_startup_config import CONFIG_RECEIPT, sha, stage_lock
from tor_recovery_pg_config_verify import verify_prepared_files
import tor_recovery_pg_run as replay


def fixture_run(work, label, target, timeline):
    uid = os.geteuid()
    if uid == 0 or uid != os.getuid() or pwd.getpwuid(uid).pw_name != "postgres":
        raise Refused("fixture requires the existing postgres OS identity")
    # prepare enforces the disposable container marker, canonical random /tmp
    # path, removed source, exact fixture layout and new stage/config evidence.
    root, args, selection, uid, gid = prepare(work, label, target, timeline)
    expected_commands = replay.commands(root, args)
    actual_invoke = replay.invoke
    prefix = [str(replay.RUNUSER), "-u", "postgres", "--"]

    def same_identity(command, timeout, capture=False):
        if command not in expected_commands or command[:4] != prefix:
            raise Refused("unexpected disposable PostgreSQL command")
        # No simulated DB or returned process result: execute precisely the
        # same env -i / installed PostgreSQL command as the operator helper.
        return actual_invoke(command[4:], timeout, capture=capture)

    evidence = root / "evidence"
    with stage_lock(root, uid, gid):
        verify_data_tree(root / "pgdata", uid, gid)
        verify_stage_receipt(root, selection, uid, gid, gid)
        verify_prepared_files(root, args, uid, gid, uid, gid)
        # Local fixture-only adaptation. execute()/main() are deliberately
        # not called and their root/CI/host checks are never changed.
        with patch.object(replay, "invoke", side_effect=same_identity):
            replay.run_paused(root, args, uid, gid, uid, gid)
        raw = private_read(evidence / replay.REPLAY_RECEIPT, uid, gid, (0o600,), 16384)
        report = json.loads(raw, object_pairs_hook=unique_fields)
        expected = {
            "schema_version": 1, "scope": "paused_replay_observed_then_stopped_only",
            "backup_label": label, "target_utc": args.target_utc,
            "timeline": timeline, "pg_port": args.pg_port,
            "stage_receipt_sha256": sha(private_read(evidence / STAGE_RECEIPT, uid, gid, (0o600,), 16384)),
            "config_receipt_sha256": sha(private_read(evidence / CONFIG_RECEIPT, uid, gid, (0o600,), 16384)),
            "stable_paused_replay_observed": True,
            "postgres_shutdown_confirmed": True, "recovery_signal_preserved": True,
            "exact_target_transaction_boundary_verified": False,
            "promotion_authorized": False, "recovery_accepted": False,
        }
        witness = replay.parse_witness(json.dumps([
            True, "paused", True, report.get("replay_lsn"),
            report.get("last_replayed_transaction_utc"),
        ]), args.target_utc)
        expected.update(witness)
        if json.dumps(report, sort_keys=True) != json.dumps(expected, sort_keys=True):
            raise Refused("unexpected disposable paused-replay receipt")


def main():
    if len(sys.argv) != 5:
        return 2
    try:
        fixture_run(Path(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4]))
    except (Refused, OSError, ValueError, KeyError):
        print("I11_PITR_OPERATOR_HELPERS_REFUSED", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
