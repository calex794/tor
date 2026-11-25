"""RECOVERY-09a: completed configuration verification, negative fixtures."""
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tor_recovery_pg import STAGE_RECEIPT, Refused
from tor_recovery_pg_startup_config import (
    ATTEMPT, CONFIG_RECEIPT, ORIGINAL_AUTO, STARTUP_DIR, TEMP_AUTO, prepare_files,
)
from tor_recovery_pg_config_verify import verify, verify_prepared_files
from test_tor_recovery_pg_startup_config import args, fixture, private_file


class RecoveryPgConfigVerifyTests(unittest.TestCase):
    def prepared(self, root):
        uid, gid = os.geteuid(), os.getegid()
        fixture(root)
        prepare_files(root, args(), uid, gid, uid, gid)
        return uid, gid

    def check(self, root, uid, gid, selection=None):
        verify_prepared_files(root, selection or args(), uid, gid, uid, gid)

    def test_completed_configuration_read_only(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            uid, gid = self.prepared(root)
            before = {p.relative_to(root): p.read_bytes()
                      for p in root.rglob("*") if p.is_file()}
            self.check(root, uid, gid)
            after = {p.relative_to(root): p.read_bytes()
                     for p in root.rglob("*") if p.is_file()}
            self.assertEqual(before, after)

    def test_rejects_tampered_config_even_with_recomputed_receipt_digest(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            uid, gid = self.prepared(root)
            config = root / "config" / STARTUP_DIR / "postgresql.conf"
            config.write_bytes(config.read_bytes().replace(
                b"recovery_target_action = 'pause'", b"recovery_target_action = 'promote'"))
            receipt = root / "evidence" / CONFIG_RECEIPT
            altered = json.loads(receipt.read_bytes())
            altered["config_sha256"]["postgresql.conf"] = hashlib.sha256(config.read_bytes()).hexdigest()
            receipt.write_text(json.dumps(altered, sort_keys=True) + "\n")
            with self.assertRaises(Refused):
                self.check(root, uid, gid)

    def test_refuses_incomplete_changed_or_extra_files(self):
        def symlink_hba(root):
            path = root / "config" / STARTUP_DIR / "pg_hba.conf"
            path.unlink()
            path.symlink_to(root / "evidence" / ORIGINAL_AUTO)
        for damage in (
            lambda r: (r / "evidence" / CONFIG_RECEIPT).unlink(),
            lambda r: (r / "evidence" / ATTEMPT).write_bytes(b"changed"),
            lambda r: (r / "evidence" / ORIGINAL_AUTO).write_bytes(b"changed"),
            lambda r: (r / "evidence" / STAGE_RECEIPT).write_bytes(b"changed"),
            lambda r: (r / "evidence" / CONFIG_RECEIPT).write_bytes(b"{}"),
            lambda r: private_file(r / "pgdata/postgresql.auto.conf", b"include='unsafe'"),
            lambda r: private_file(r / "pgdata" / TEMP_AUTO, b""),
            lambda r: (r / "pgdata/postgresql.auto.conf").chmod(0o400),
            lambda r: private_file(r / "config" / STARTUP_DIR / "extra.conf", b""),
            lambda r: (r / "config" / STARTUP_DIR / "pg_hba.conf").chmod(0o666),
            symlink_hba,
        ):
            with self.subTest(damage=damage), tempfile.TemporaryDirectory() as folder:
                root = Path(folder).resolve()
                uid, gid = self.prepared(root)
                damage(root)
                with self.assertRaises((Refused, OSError)):
                    self.check(root, uid, gid)

    def test_independent_target_and_resources_cannot_drift(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            uid, gid = self.prepared(root)
            for selection in (
                args(timeline=4), args(pg_port=15433),
                args(target_utc="2026-11-23T11:58:00Z"),
                args(max_connections=200),
            ):
                with self.subTest(selection=selection), self.assertRaises(Refused):
                    self.check(root, uid, gid, selection)

    def test_failed_preflight_blocks_verify(self):
        with patch("tor_recovery_pg_config_verify.validate_preflight_host",
                   return_value=(123, 123)), \
             patch("tor_recovery_pg_config_verify.stage_lock"), \
             patch("tor_recovery_pg_config_verify.preflight",
                   side_effect=Refused("fixture refusal")), \
             patch("tor_recovery_pg_config_verify.verify_prepared_files") as check:
            with self.assertRaises(Refused):
                verify(args())
            check.assert_not_called()


if __name__ == "__main__":
    unittest.main()
