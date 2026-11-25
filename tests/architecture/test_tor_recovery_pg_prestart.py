"""RECOVERY-07: isolated local file fixtures; never launch PostgreSQL."""
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
from tor_recovery_pg import STAGE_RECEIPT, Refused, stage_receipt, write_stage_receipt
from tor_recovery_pg_prestart import verify_data_tree, verify_stage_receipt, main


def file(path, data=b""):
    path.write_bytes(data)
    path.chmod(0o600)


def fixture(root):
    pg = root / "pgdata"
    for name in ("pgdata", "pgdata/global", "pgdata/pg_wal", "pgdata/pg_tblspc"):
        path = root / name
        path.mkdir(mode=0o700)
        path.chmod(0o700)
    file(pg / "PG_VERSION", b"18\n")
    file(pg / "global/pg_control", b"fixture-only")
    file(pg / "recovery.signal")
    file(pg / "postgresql.auto.conf", b"untrusted text: never execute")
    return pg


def args():
    return SimpleNamespace(
        source_machine_id="a" * 32, target_machine_id="b" * 32,
        pg_port=15432, backup_label="20261120-020000F",
        target_utc="2026-11-23T11:59:00Z", incident_utc="2026-11-23T12:00:00Z",
        timeline=3,
    )


class RecoveryPgPrestartTests(unittest.TestCase):
    def test_staged_tree_read_only_and_inherited_config_untouched(self):
        with tempfile.TemporaryDirectory() as folder:
            pg = fixture(Path(folder).resolve())
            original = (pg / "postgresql.auto.conf").read_bytes()
            verify_data_tree(pg, os.geteuid(), os.getegid())
            self.assertEqual((pg / "postgresql.auto.conf").read_bytes(), original)

    def test_unsafe_links_special_files_modes_and_tablespaces_are_refused(self):
        def hardlink(pg):
            os.link(pg / "global/pg_control", pg / "second-control")
        def table(pg):
            file(pg / "pg_tblspc" / "12345", b"unexpected")
        for damage in (
            lambda pg: (pg / "escape").symlink_to("/etc/passwd"),
            hardlink,
            lambda pg: os.mkfifo(pg / "pipe", 0o600),
            lambda pg: (pg / "global/pg_control").chmod(0o666),
            lambda pg: file(pg / "postmaster.pid"),
            lambda pg: file(pg / "standby.signal"),
            lambda pg: file(pg / "recovery.signal", b"unexpected"),
            lambda pg: file(pg / "PG_VERSION", b"17"),
            table,
        ):
            with tempfile.TemporaryDirectory() as folder:
                pg = fixture(Path(folder).resolve())
                damage(pg)
                with self.subTest(damage=damage), self.assertRaises(Refused):
                    verify_data_tree(pg, os.geteuid(), os.getegid())

    def test_receipt_pins_selection_and_does_not_contain_secrets(self):
        data = {
            "endpoint": "backup.example.invalid", "bucket": "tor-dr-archive",
            "region": "test-region-1", "repo_path": "/tor-postgres-18",
            "access_key": "synthetic-access", "secret_key": "synthetic-secret",
            "cipher_pass": "c" * 48,
        }
        selected = stage_receipt(args(), data, Path("/srv/tor-recovery"))
        self.assertEqual(selected["scope"], "physical_files_staged_only")
        self.assertEqual(selected["timeline"], 3)
        for key in ("access_key", "secret_key", "cipher_pass"):
            self.assertNotIn(data[key], json.dumps(selected))
        changed = args()
        changed.timeline = 4
        self.assertNotEqual(selected, stage_receipt(changed, data, Path("/srv/tor-recovery")))

    def test_receipt_and_config_reject_changes_and_include_injection(self):
        uid, gid = os.geteuid(), os.getegid()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            for name in ("evidence", "config", "config/pgbackrest-empty"):
                (root / name).mkdir(mode=0o750)
                (root / name).chmod(0o750)
            cfg = root / "config/pgbackrest-recovery.conf"
            file(cfg, b"synthetic config")
            cfg.chmod(0o640)
            receipt = {"schema_version": 1, "timeline": 3,
                       "repository_config_sha256": hashlib.sha256(cfg.read_bytes()).hexdigest()}
            write_stage_receipt(root, receipt)
            verify_stage_receipt(root, receipt, uid, gid, gid)
            with self.assertRaises(FileExistsError):
                write_stage_receipt(root, receipt)
            with self.assertRaises(Refused):
                verify_stage_receipt(root, dict(receipt, timeline=4), uid, gid, gid)
            cfg.write_bytes(b"changed")
            with self.assertRaises(Refused):
                verify_stage_receipt(root, receipt, uid, gid, gid)
            cfg.write_bytes(b"synthetic config")
            file(root / "config/pgbackrest-empty/evil.conf", b"extra")
            with self.assertRaises(Refused):
                verify_stage_receipt(root, receipt, uid, gid, gid)
            (root / "config/pgbackrest-empty/evil.conf").unlink()
            record = root / "evidence" / STAGE_RECEIPT
            record.write_text(json.dumps(dict(receipt, schema_version=True)))
            with self.assertRaises(Refused):
                verify_stage_receipt(root, receipt, uid, gid, gid)

    def test_ci_cannot_run_operator_preflight(self):
        argv = ["prestart", "--source-machine-id", "a" * 32,
                "--target-machine-id", "b" * 32, "--backup-label", "20261120-020000F",
                "--target-utc", "2026-11-23T11:59:00Z",
                "--incident-utc", "2026-11-23T12:00:00Z", "--timeline", "3",
                "--endpoint", "backup.example.invalid", "--bucket", "tor-dr-archive",
                "--repo-path", "/tor-postgres-18",
                "--operator-confirm-quiescent-isolated-host"]
        with patch.object(sys, "argv", argv), patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}), \
                patch("tor_recovery_pg_prestart.verify_data_tree") as scan:
            self.assertEqual(main(), 2)
            scan.assert_not_called()


if __name__ == "__main__":
    unittest.main()
