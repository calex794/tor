"""RECOVERY-02: actual local workspace and host boundary negative tests."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tor_recovery_host import (
    NAME, POSTGRES_DIRS, PRIVATE_DIRS, Refused, check_host, prepare_layout,
    reject_live_markers, validate_target,
)

SOURCE = "a" * 32
TARGET = "b" * 32


class RecoveryHostTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.parent = Path(self.tmp.name).resolve()

    def prepare(self, parent=None, port=15432):
        return prepare_layout(parent or self.parent, os.geteuid(), os.getegid(), port)

    def test_distinct_approved_host_and_nonproduction_port(self):
        validate_target(SOURCE, TARGET, TARGET, 15432)
        for source, expected, actual, port in (
            (TARGET, TARGET, TARGET, 15432),
            (SOURCE, TARGET, "c" * 32, 15432),
            ("0" * 32, TARGET, TARGET, 15432),
            ("bad", TARGET, TARGET, 15432),
            (SOURCE, TARGET, TARGET, 5432),
            (SOURCE, TARGET, TARGET, 16000),
        ):
            with self.subTest(port=port), self.assertRaises(Refused):
                validate_target(source, expected, actual, port)

    def test_existing_install_and_dangling_marker_refused(self):
        marker = self.parent / "existing"
        reject_live_markers([marker])
        marker.mkdir()
        with self.assertRaisesRegex(Refused, "existing TOR"):
            reject_live_markers([marker])
        marker.rmdir()
        marker.symlink_to(self.parent / "missing")
        with self.assertRaises(Refused):
            reject_live_markers([marker])

    def test_prepare_empty_data_and_private_workspace(self):
        root = self.prepare()
        self.assertEqual(root, self.parent / NAME)
        self.assertEqual(root.stat().st_mode & 0o777, 0o710)
        for name in POSTGRES_DIRS + PRIVATE_DIRS:
            self.assertEqual(list((root / name).iterdir()), [])
            self.assertEqual((root / name).stat().st_mode & 0o777, 0o700)
        config = root / "config"
        for file in config.iterdir():
            self.assertEqual(file.stat().st_mode & 0o777, 0o640)
        pg = (config / "postgresql.conf").read_text()
        self.assertIn("listen_addresses = ''", pg)
        self.assertIn("port = 15432", pg)
        self.assertIn("archive_mode = off", pg)
        self.assertIn(f"data_directory = '{root}/pgdata'", pg)
        hba = (config / "pg_hba.conf").read_text()
        self.assertIn("local all postgres peer", hba)
        self.assertIn("host all all 0.0.0.0/0 reject", hba)
        self.assertIn("host all all ::/0 reject", hba)
        self.assertFalse((root / "pgdata" / "PG_VERSION").exists())

    def test_never_overwrite_existing_workspace(self):
        root = self.prepare()
        witness = root / "pgdata" / "retained"
        witness.write_bytes(b"do not overwrite")
        with self.assertRaisesRegex(Refused, "already exists"):
            self.prepare()
        self.assertEqual(witness.read_bytes(), b"do not overwrite")

    def test_unsafe_parent_and_symlinks_refused(self):
        other = self.parent / "other"
        other.mkdir()
        other.chmod(0o777)
        with self.assertRaisesRegex(Refused, "workspace parent"):
            self.prepare(parent=other)
        self.assertFalse((other / NAME).exists())
        link = self.parent / "link"
        link.symlink_to(other)
        with self.assertRaises(Refused):
            self.prepare(parent=link)
        (self.parent / NAME).symlink_to(other)
        with self.assertRaisesRegex(Refused, "already exists"):
            self.prepare()
        self.assertEqual(list(other.iterdir()), [])

    def test_ci_cannot_enter_operational_host_check(self):
        with patch("tor_recovery_host.os.geteuid", return_value=0), \
                patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}):
            with self.assertRaisesRegex(Refused, "prohibited in CI"):
                check_host(SOURCE, TARGET, 15432)


if __name__ == "__main__":
    unittest.main()
