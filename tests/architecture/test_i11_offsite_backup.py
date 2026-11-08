"""I-11/DR-01: backup scripts are explicitly operator-only, not production CI."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
BACKUP = ROOT / "ops" / "backup-pg18-offsite.sh"
RESTORE = ROOT / "ops" / "verify-pg18-offsite-restore.sh"
TIMER = ROOT / "ops" / "tor-offsite-backup.timer.example"
SERVICE = ROOT / "ops" / "tor-offsite-backup.service.example"


class OffsiteBackupBoundaries(unittest.TestCase):
    def test_shell_is_valid(self):
        for script in (BACKUP, RESTORE):
            result = subprocess.run(["bash", "-n", str(script)],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_backup_is_remote_encrypted_and_fail_closed(self):
        src = BACKUP.read_text(encoding="utf-8")
        for required in (
            "root:root:600", "s3:https://", "restic backup",
            "export RESTIC_REPOSITORY",
            "--stdin-from-command", "pg_dump", "flock -n",
            "--format=custom", "TOR_OFFSITE_BACKUP_OK",
        ):
            self.assertIn(required, src)
        self.assertNotIn("restic init", src)
        self.assertNotIn("GITHUB_TOKEN", src)
        self.assertNotIn("--insecure-tls", src)

    def test_restore_never_points_to_production_database(self):
        src = RESTORE.read_text(encoding="utf-8")
        for required in (
            "TOR_DR_RESTORE_URL", "15432 <= u.port <= 15999",
            "tor_dr_", "restore target is nonempty",
            "restic dump", "pg_restore", "TOR_OFFSITE_RESTORE_VERIFIED",
        ):
            self.assertIn(required, src)
        self.assertNotIn("DROP DATABASE", src)
        self.assertNotIn("restic forget", src)

    def test_timer_is_manual_and_does_not_contact_live_server_from_ci(self):
        timer = TIMER.read_text(encoding="utf-8")
        service = SERVICE.read_text(encoding="utf-8")
        self.assertIn("OnCalendar=*-*-* 03:20:00", timer)
        self.assertIn("Persistent=true", timer)
        self.assertIn("ExecStart=/usr/local/sbin/tor-offsite-backup", service)
        self.assertNotIn("ExecStartPre=restic init", service)


if __name__ == "__main__":
    unittest.main()
