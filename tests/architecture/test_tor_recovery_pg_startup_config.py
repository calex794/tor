"""RECOVERY-08: disposable configuration fixtures, no PostgreSQL execution."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tor_recovery_pg import STAGE_RECEIPT, Refused
from tor_recovery_pg_startup_config import (
    ATTEMPT, CONFIG_RECEIPT, ORIGINAL_AUTO, STARTUP_DIR, TEMP_AUTO,
    execute, prepare_files, render_startup, resource_settings, stage_lock,
)


def args(**changes):
    data = dict(
        backup_label="20261120-020000F",
        target_utc="2026-11-23T11:59:00Z",
        incident_utc="2026-11-23T12:00:00Z",
        timeline=3, pg_port=15432, max_connections=100,
        max_prepared_transactions=0, max_locks_per_transaction=64,
        max_wal_senders=10, max_worker_processes=8,
        shared_buffers_mb=128, prepare_config=False,
    )
    data.update(changes)
    return SimpleNamespace(**data)


def private_file(path, content):
    path.write_bytes(content)
    path.chmod(0o600)


def fixture(root):
    for name in ("config", "pgdata", "evidence"):
        (root / name).mkdir(mode=0o700)
        (root / name).chmod(0o700)
    private_file(root / "evidence" / STAGE_RECEIPT, b'{"stage":"fixture"}')
    private_file(root / "evidence" / "pg-restore.lock", b"")
    private_file(root / "pgdata/postgresql.auto.conf",
                 b"include = '/untrusted/example.conf'\n")


class RecoveryPgConfigTests(unittest.TestCase):
    def test_fresh_config_pins_pause_and_disables_live_connections(self):
        files = render_startup(Path("/srv/tor-recovery"), args())
        config = files["postgresql.conf"].decode()
        for expected in (
            "listen_addresses = ''", "archive_mode = off",
            "archive_library = ''", "primary_conninfo = ''",
            "shared_preload_libraries = ''", "session_preload_libraries = ''",
            "local_preload_libraries = ''", "recovery_end_command = ''",
            "archive_cleanup_command = ''", "hot_standby = on",
            "recovery_target_time = '2026-11-23 11:59:00+00'",
            "recovery_target_timeline = '3'", "recovery_target_inclusive = off",
            "recovery_target_action = 'pause'",
            "--config-include-path=/srv/tor-recovery/config/pgbackrest-empty",
            'archive-get "%f" "%p"', "/usr/bin/env -i",
        ):
            self.assertIn(expected, config)
        self.assertNotIn("include =", config)
        self.assertNotIn("archive-push", config)
        self.assertIn(b"local all postgres peer", files["pg_hba.conf"])
        self.assertIn(b"local replication all reject", files["pg_hba.conf"])

    def test_untrusted_path_target_and_resource_values_refused(self):
        for path in (Path("/tmp/with space"), Path("/tmp/../escape")):
            with self.assertRaises(Refused):
                render_startup(path, args())
        for changes in (
            {"pg_port": 5432}, {"timeline": 0}, {"max_connections": True},
            {"max_connections": 0}, {"max_wal_senders": 100},
            {"max_worker_processes": 129}, {"shared_buffers_mb": None},
            {"target_utc": "2026-11-23T12:00:00Z"},
        ):
            with self.subTest(changes=changes), self.assertRaises(Refused):
                render_startup(Path("/srv/tor-recovery"), args(**changes))
        self.assertEqual(resource_settings(args())["max_connections"], 100)

    def test_private_archive_then_empty_auto_and_exact_receipt(self):
        uid, gid = os.geteuid(), os.getegid()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            fixture(root)
            auto = root / "pgdata/postgresql.auto.conf"
            original = auto.read_bytes()
            prepare_files(root, args(), uid, gid, uid, gid)
            self.assertEqual(auto.read_bytes(), b"")
            self.assertEqual((root / "evidence" / ORIGINAL_AUTO).read_bytes(), original)
            self.assertEqual((root / "evidence" / ORIGINAL_AUTO).stat().st_mode & 0o777, 0o600)
            receipt = json.loads((root / "evidence" / CONFIG_RECEIPT).read_bytes())
            self.assertIs(receipt["startup_authorized"], False)
            self.assertEqual(receipt["original_auto_conf_sha256"],
                             hashlib.sha256(original).hexdigest())
            self.assertEqual(receipt["stage_receipt_sha256"],
                             hashlib.sha256((root / "evidence" / STAGE_RECEIPT).read_bytes()).hexdigest())
            for name, digest in receipt["config_sha256"].items():
                file = root / "config" / STARTUP_DIR / name
                self.assertEqual(hashlib.sha256(file.read_bytes()).hexdigest(), digest)
                self.assertEqual(file.stat().st_mode & 0o777, 0o640)
            self.assertFalse((root / "pgdata" / TEMP_AUTO).exists())
            with self.assertRaises(Refused):
                prepare_files(root, args(), uid, gid, uid, gid)

    def test_partial_failure_retains_evidence_and_never_claims_prepared(self):
        uid, gid = os.geteuid(), os.getegid()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            fixture(root)
            auto = root / "pgdata/postgresql.auto.conf"
            original = auto.read_bytes()
            with patch("tor_recovery_pg_startup_config.os.replace",
                       side_effect=OSError("fixture failure")):
                with self.assertRaises(OSError):
                    prepare_files(root, args(), uid, gid, uid, gid)
            self.assertEqual(auto.read_bytes(), original)
            self.assertEqual((root / "evidence" / ORIGINAL_AUTO).read_bytes(), original)
            self.assertTrue((root / "evidence" / ATTEMPT).exists())
            self.assertFalse((root / "evidence" / CONFIG_RECEIPT).exists())
            with self.assertRaises(Refused):
                prepare_files(root, args(), uid, gid, uid, gid)

    def test_shared_lock_refuses_concurrent_or_symlink_attempt(self):
        uid, gid = os.geteuid(), os.getegid()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            fixture(root)
            with stage_lock(root, uid, gid):
                with self.assertRaises(BlockingIOError):
                    with stage_lock(root, uid, gid):
                        self.fail("concurrent lock acquired")
            path = root / "evidence/pg-restore.lock"
            path.unlink()
            path.symlink_to(root / "evidence" / STAGE_RECEIPT)
            with self.assertRaises(OSError):
                with stage_lock(root, uid, gid):
                    self.fail("symlink lock acquired")

    def test_default_and_failed_preflight_cannot_prepare(self):
        with patch("tor_recovery_pg_startup_config.validate_preflight_host",
                   return_value=(123, 123)), \
             patch("tor_recovery_pg_startup_config.stage_lock"), \
             patch("tor_recovery_pg_startup_config.preflight") as preflight, \
             patch("tor_recovery_pg_startup_config.reject_prior_attempt"), \
             patch("tor_recovery_pg_startup_config.prepare_files") as prepare:
            execute(args())
            preflight.assert_called_once()
            prepare.assert_not_called()
            preflight.side_effect = Refused("fixture refusal")
            with self.assertRaises(Refused):
                execute(args(prepare_config=True))
            prepare.assert_not_called()


if __name__ == "__main__":
    unittest.main()
