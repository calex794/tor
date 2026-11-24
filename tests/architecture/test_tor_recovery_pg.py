"""RECOVERY-03a: fail-closed checkpoint, repository and execution boundaries."""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tor_recovery_pg import (
    Refused, checkpoint, repository_settings, restore_command, render_config,
    unique_fields, checked_directory, main,
)

LABEL = "20261120-020000F_20261123-020000D"
TARGET = "2026-11-23T11:59:00Z"
INCIDENT = "2026-11-23T12:00:00Z"
DATA = {
    "endpoint": "backup.example.invalid", "bucket": "tor-dr-archive",
    "region": "test-region-1", "repo_path": "/tor-postgres-18",
    "access_key": "synthetic-access-1234", "secret_key": "synthetic-secret-1234",
    "cipher_pass": "c" * 48,
}


class RecoveryPgTests(unittest.TestCase):
    def test_pinned_label_timeline_and_exclusive_target(self):
        target = checkpoint(LABEL, TARGET, INCIDENT, 3)
        self.assertEqual(target, "2026-11-23 11:59:00+00")
        cmd = restore_command(Path("/srv/tor-recovery"), LABEL, target, 3)
        self.assertEqual(cmd[:4], ["/usr/sbin/runuser", "-u", "postgres", "--"])
        for expected in (
            "--set=" + LABEL, "--target=" + target, "--type=time",
            "--target-timeline=3", "--target-exclusive", "--target-action=shutdown",
            "--pg1-path=/srv/tor-recovery/pgdata",
            "--tablespace-map-all=/srv/tor-recovery/tablespaces",
            "--config-include-path=/srv/tor-recovery/config/pgbackrest-empty",
        ):
            self.assertIn(expected, cmd)
        for forbidden in ("--force", "--delta", "--link-all", "--set=latest"):
            self.assertNotIn(forbidden, cmd)
        self.assertEqual(cmd[-1], "restore")
        self.assertNotIn(DATA["secret_key"], " ".join(cmd))

    def test_refuses_ambiguous_or_postincident_targets(self):
        for label, target, incident, timeline in (
            ("latest", TARGET, INCIDENT, 3),
            (LABEL, INCIDENT, INCIDENT, 3),
            (LABEL, "2026-11-23T12:01:00Z", INCIDENT, 3),
            (LABEL, "2026-11-23 11:59:00", INCIDENT, 3),
            (LABEL, "2026-02-30T11:59:00Z", INCIDENT, 3),
            (LABEL, TARGET, INCIDENT, 0),
            (LABEL, TARGET, INCIDENT, True),
        ):
            with self.subTest(label=label, target=target, timeline=timeline):
                with self.assertRaises(Refused):
                    checkpoint(label, target, incident, timeline)

    def verify_repository(self, data):
        return repository_settings(data, DATA["endpoint"], DATA["bucket"],
                                   DATA["repo_path"])

    def test_repository_pin_and_strict_allowlist(self):
        self.assertEqual(self.verify_repository(DATA), DATA)
        for bad in (
            dict(DATA, bucket="tore"),
            dict(DATA, bucket="other-backup"),
            dict(DATA, endpoint="attacker.example.invalid"),
            dict(DATA, repo_path="/other"),
            dict(DATA, repo_path="/../tor"),
            dict(DATA, secret_key="abc\nrepo1-type=posix"),
            dict(DATA, cipher_pass="short"),
            dict(DATA, **{"pg1-path": "/var/lib/postgresql/18/main"}),
        ):
            with self.subTest(keys=tuple(bad)), self.assertRaises(Refused):
                self.verify_repository(bad)
        with self.assertRaises(Refused):
            unique_fields([("bucket", "first"), ("bucket", "second")])

    def test_config_pins_paths_tls_and_no_remote_pg_host(self):
        cfg = render_config(DATA, Path("/srv/tor-recovery"), 15432)
        self.assertIn("repo1-storage-verify-tls=y", cfg)
        self.assertIn("repo1-cipher-type=aes-256-cbc", cfg)
        self.assertIn("pg1-path=/srv/tor-recovery/pgdata", cfg)
        self.assertIn("pg1-port=15432", cfg)
        self.assertIn("pg1-socket-path=/srv/tor-recovery/pgsocket", cfg)
        self.assertNotIn("pg1-host=", cfg)
        self.assertNotIn("archive-push", cfg)

    def test_nonempty_and_symlink_destination_refused(self):
        import os
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder).resolve()
            path.chmod(0o700)
            checked_directory(path, os.geteuid(), os.getegid(), 0o700, empty=True)
            (path / "existing").write_text("never overwrite")
            with self.assertRaisesRegex(Refused, "not empty"):
                checked_directory(path, os.geteuid(), os.getegid(), 0o700, empty=True)
            link = path / "symlink"
            link.symlink_to(path)
            with self.assertRaises(Refused):
                checked_directory(link, os.geteuid(), os.getegid(), 0o700)

    def test_default_is_plan_never_calls_restore(self):
        argv = ["tor_recovery_pg", "--source-machine-id", "a" * 32,
                "--target-machine-id", "b" * 32, "--backup-label", LABEL,
                "--target-utc", TARGET, "--incident-utc", INCIDENT, "--timeline", "3",
                "--endpoint", DATA["endpoint"], "--bucket", DATA["bucket"],
                "--repo-path", DATA["repo_path"]]
        with patch.object(sys, "argv", argv), patch("tor_recovery_pg.stage_backup") as stage:
            self.assertEqual(main(), 0)
            stage.assert_not_called()


if __name__ == "__main__":
    unittest.main()
