"""I-11 DR-03 static contract: scripts cannot silently deploy or delete backups."""
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BACKUP = ROOT / "ops/backup-s3-offsite.sh"
RESTORE = ROOT / "ops/verify-s3-offsite-restore.sh"


class Dr03OperatorSafety(unittest.TestCase):
    def test_syntax(self):
        for script in (BACKUP, RESTORE):
            check = subprocess.run(["bash", "-n", str(script)],
                                   capture_output=True, text=True)
            self.assertEqual(check.returncode, 0, check.stderr)

    def test_backup_is_manual_encrypted_and_immutable(self):
        data = BACKUP.read_text(encoding="utf-8")
        for token in ("[[ ! -v GITHUB_ACTIONS ]]", "root:root:600",
                      "tor-primary-ro:", "tor-offsite-crypt:",
                      "--immutable", "--download --one-way",
                      "age -r", "TOR_DR_SECRET_BUNDLE",
                      "recovery-keys/", "cmp -s", "flock -n",
                      "config dump", 'c.get("type") == "crypt"',
                      "/run/tor-dr03", "symlink lock refused",
                      "TOR_DR03_S3_AND_KEYS_OFFSITE_OK"):
            self.assertIn(token, data)
        for forbidden in ("rclone sync", "rclone move", "rclone delete",
                          "rclone purge", "systemctl", "ssh ", "curl ",
                          "GITHUB_TOKEN", "pg_restore"):
            self.assertNotIn(forbidden, data)

    def test_restore_is_read_only_and_checks_offline_proof(self):
        data = RESTORE.read_text(encoding="utf-8")
        for token in ("[[ ! -v GITHUB_ACTIONS ]]", "tor-offsite-read:",
                      "TOR_DR_AGE_IDENTITY", "TOR_DR_EXPECTED_OBJECT_SHA256",
                      "TOR_DR_EXPECTED_SECRET_SHA256", "age -d -i",
                      "private/", "sha256sum", "config dump",
                      'c.get("type") == "crypt"', "stat -f -c",
                      "/run/tor-dr03-restore",
                      "TOR_DR03_PRIVATE_OBJECT_AND_OFFLINE_KEY_RESTORED"):
            self.assertIn(token, data)
        for forbidden in ("rclone sync", "rclone move", "rclone delete",
                          "rclone purge", "systemctl", "DROP DATABASE",
                          "CREATE DATABASE", "ssh ", "curl ",
                          "keys.txt", " -o \"$work/keys"):
            self.assertNotIn(forbidden, data)


if __name__ == "__main__":
    unittest.main()
