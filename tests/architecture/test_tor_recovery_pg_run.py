"""RECOVERY-09c: command, witness and shutdown tests; no real operator launch."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tor_recovery_host import Refused
import tor_recovery_pg_run as run
from test_tor_recovery_pg_startup_config import args

STAMP = "2026-11-23T11:58:59.123456Z"
GOOD = [True, "paused", True, "0/16B6C50", STAMP]


def selection(**kwargs):
    values = dict(replay_timeout_seconds=60, run_paused_replay=False,
                  operator_confirm_reviewed_isolated_execution=False)
    values.update(kwargs)
    return args(**values)


def result(status=0, stdout=""):
    return subprocess.CompletedProcess([], status, stdout, "")


class RecoveryPgRunTests(unittest.TestCase):
    def test_fixed_commands_clean_environment_and_no_promotion(self):
        start, query, stop = run.commands(Path("/srv/tor-recovery"), selection())
        for command in (start, query, stop):
            self.assertEqual(command[:6], ["/usr/sbin/runuser", "-u", "postgres",
                                            "--", "/usr/bin/env", "-i"])
            self.assertIn("HOME=/nonexistent", command)
        self.assertEqual(start[-1], "start")
        self.assertIn("-c config_file=/srv/tor-recovery/config/pg-startup/postgresql.conf", start)
        self.assertIn("/usr/lib/postgresql/18/bin/postgres", start)
        self.assertEqual(stop[-1], "stop")
        self.assertIn("fast", stop)
        self.assertIn("-X", query)
        self.assertIn("-w", query)
        self.assertIn("/srv/tor-recovery/pgsocket", query)
        sql = query[-1]
        for part in ("pg_catalog.pg_is_in_recovery()", "pg_get_wal_replay_pause_state()",
                     "recovery_target_inclusive", "recovery_target_action",
                     "pg_last_wal_replay_lsn()", "pg_last_xact_replay_timestamp()"):
            self.assertIn(part, sql)
        self.assertNotIn("pg_promote(", sql)
        self.assertNotIn("pg_wal_replay_resume(", sql)
        self.assertNotIn("promote", start + stop)
        with patch.object(run.subprocess, "run", return_value=result()) as call:
            run.invoke(start, 70)
            self.assertEqual(call.call_args.kwargs["env"], run.CLEAN_ENV)
            self.assertIs(call.call_args.kwargs["check"], False)

    def test_witness_strictly_paused_and_before_target(self):
        expected = {"replay_lsn": GOOD[3], "last_replayed_transaction_utc": STAMP}
        self.assertEqual(run.parse_witness(json.dumps(GOOD), selection().target_utc), expected)
        for state in ("not paused", "pause requested"):
            row = [True, state, True, None, None]
            self.assertIsNone(run.parse_witness(json.dumps(row), selection().target_utc))
        for row in ([False, "primary", False, None, None],
                    [True, "paused", False, GOOD[3], STAMP],
                    [1, "paused", True, GOOD[3], STAMP],
                    [True, "paused", 1, GOOD[3], STAMP],
                    [True, "other", True, GOOD[3], STAMP],
                    [True, "paused", True, "0/0", STAMP],
                    [True, "paused", True, GOOD[3], None],
                    [True, "paused", True, GOOD[3], "2026-11-23T11:59:00.000000Z"],
                    [True, "paused", True, GOOD[3], "2026-11-23T12:00:00.000000Z"]):
            with self.subTest(row=row), self.assertRaises(Refused):
                run.parse_witness(json.dumps(row), selection().target_utc)

    def test_stable_witness_requires_two_matching_samples(self):
        with patch.object(run, "invoke", side_effect=[
            result(2), result(stdout=json.dumps([True, "pause requested", True, None, None])),
            result(stdout=json.dumps(GOOD)), result(stdout=json.dumps(GOOD)),
        ]) as invoke, patch.object(run.time, "sleep"), patch.object(run.time, "monotonic", return_value=0):
            self.assertEqual(run.wait_paused([], selection().target_utc, 60)["replay_lsn"], GOOD[3])
            self.assertEqual(invoke.call_count, 4)

    def test_plan_and_unconfirmed_run_do_not_start(self):
        with patch.object(run, "validate_preflight_host", return_value=(123, 123)), \
             patch.object(run, "stage_lock"), patch.object(run, "preflight"), \
             patch.object(run, "verify_prepared_files") as verify, \
             patch.object(run, "reject_launch_artifacts"), \
             patch.object(run, "check_runtime"), patch.object(run, "run_paused") as launch:
            run.execute(selection())
            verify.assert_called_once()
            launch.assert_not_called()
            with self.assertRaises(Refused):
                run.execute(args(replay_timeout_seconds=60, run_paused_replay=True,
                                 operator_confirm_reviewed_isolated_execution=False))
            launch.assert_not_called()

    def test_preflight_failure_never_launches(self):
        with patch.object(run, "validate_preflight_host", side_effect=Refused("fixture refusal")), \
             patch.object(run, "invoke") as invoke:
            with self.assertRaises(Refused):
                run.execute(selection())
            invoke.assert_not_called()

    def fixture(self, root):
        for name in ("pgdata", "pgsocket", "pglog", "evidence"):
            (root / name).mkdir(mode=0o700)
        (root / "pgdata/recovery.signal").touch(mode=0o600)
        for name in (run.STAGE_RECEIPT, run.CONFIG_RECEIPT):
            path = root / "evidence" / name
            path.write_bytes(b"fixture receipt")
            path.chmod(0o600)
        return os.geteuid(), os.getegid()

    def test_receipt_only_after_successful_shutdown(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            uid, gid = self.fixture(root)
            witness = run.parse_witness(json.dumps(GOOD), selection().target_utc)
            with patch.object(run, "invoke", side_effect=[result(), result()]) as invoke, \
                 patch.object(run, "wait_paused", return_value=witness):
                run.run_paused(root, selection(), uid, gid, uid, gid)
            self.assertEqual([c.args[0][-1] for c in invoke.call_args_list], ["start", "stop"])
            report = json.loads((root / "evidence" / run.REPLAY_RECEIPT).read_bytes())
            self.assertIs(report["stable_paused_replay_observed"], True)
            self.assertIs(report["postgres_shutdown_confirmed"], True)
            self.assertEqual(report["stage_receipt_sha256"], run.sha(b"fixture receipt"))
            self.assertEqual(report["config_receipt_sha256"], run.sha(b"fixture receipt"))
            for key in ("exact_target_transaction_boundary_verified", "promotion_authorized",
                        "recovery_accepted"):
                self.assertIs(report[key], False)
            with self.assertRaises(Refused):
                run.reject_launch_artifacts(root)

    def test_failed_start_failed_witness_and_failed_stop_never_receipt(self):
        for start, stop, failure in ((1, 0, False), (0, 0, True), (0, 1, False)):
            with self.subTest(start=start, stop=stop, failure=failure), \
                    tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                uid, gid = self.fixture(root)
                witness = run.parse_witness(json.dumps(GOOD), selection().target_utc)
                with patch.object(run, "invoke", side_effect=[result(start), result(stop)]) as invoke, \
                     patch.object(run, "wait_paused", side_effect=Refused("no witness") if failure else None,
                                  return_value=witness):
                    with self.assertRaises(Refused):
                        run.run_paused(root, selection(), uid, gid, uid, gid)
                self.assertEqual([c.args[0][-1] for c in invoke.call_args_list], ["start", "stop"])
                self.assertTrue((root / "evidence" / run.LAUNCH_ATTEMPT).is_file())
                self.assertFalse((root / "evidence" / run.REPLAY_RECEIPT).exists())

    def test_start_timeout_still_attempts_stop(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            uid, gid = self.fixture(root)
            with patch.object(run, "invoke", side_effect=[
                    subprocess.TimeoutExpired("fixture", 60), result()]) as invoke:
                with self.assertRaises(subprocess.TimeoutExpired):
                    run.run_paused(root, selection(), uid, gid, uid, gid)
            self.assertEqual(invoke.call_args_list[-1].args[0][-1], "stop")
            self.assertFalse((root / "evidence" / run.REPLAY_RECEIPT).exists())

    def test_invalid_timeout_refused(self):
        for timeout in (False, 0, 59, 14401, "60"):
            with self.subTest(timeout=timeout), self.assertRaises(Refused):
                run.timeout_seconds(SimpleNamespace(replay_timeout_seconds=timeout))


if __name__ == "__main__":
    unittest.main()
