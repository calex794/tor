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
                      "config dump", 'x.get("type") == "crypt"',
                      "/run/tor-dr03", "symlink lock refused",
                      "TOR_DR03_S3_AND_KEYS_OFFSITE_OK"):
            self.assertIn(token, data)
        for forbidden in ("rclone sync", "rclone move", "rclone delete",
                          "rclone purge", "systemctl", "ssh ", "curl ",
                          "GITHUB_TOKEN", "pg_restore"):
            self.assertNotIn(forbidden, data)

    def test_crypt_backing_must_be_real_s3_and_not_local(self):
        import json
        cases = (
            (BACKUP, "tor-offsite-crypt", {
                "tor-primary-ro": {"type": "s3"},
                "tor-offsite-s3": {"type": "s3"},
                "tor-offsite-crypt": {"type": "crypt", "remote": "tor-offsite-s3:bucket/prefix"},
            }),
            (RESTORE, "tor-offsite-read", {
                "offsite-store": {"type": "s3"},
                "tor-offsite-read": {"type": "crypt", "remote": "offsite-store:bucket/prefix"},
            }),
        )
        for path, alias, valid in cases:
            data = path.read_text(encoding="utf-8")
            self.assertIn("config dump |", data)
            code = data.split("config dump |", 1)[1].split("python3 -c '", 1)[1].split("' ||", 1)[0]
            def check(cfg):
                return subprocess.run(
                    ["python3", "-c", code], input=json.dumps(cfg),
                    text=True, capture_output=True).returncode == 0
            with self.subTest(path=path, case="valid"):
                self.assertTrue(check(valid))
            backing = "tor-offsite-s3" if path == BACKUP else "offsite-store"
            wrong = json.loads(json.dumps(valid))
            wrong[backing]["type"] = "local"
            with self.subTest(path=path, case="local_backend"):
                self.assertFalse(check(wrong))
            wrong = json.loads(json.dumps(valid))
            wrong[alias]["remote"] = "localfolder:/home/backups"
            with self.subTest(path=path, case="unconfigured_alias"):
                self.assertFalse(check(wrong))
            wrong = json.loads(json.dumps(valid))
            wrong[alias]["remote"] = backing + ":"
            with self.subTest(path=path, case="missing_bucket"):
                self.assertFalse(check(wrong))
            wrong = json.loads(json.dumps(valid))
            wrong[alias]["type"] = "s3"
            with self.subTest(path=path, case="not_crypt"):
                self.assertFalse(check(wrong))
            if path == BACKUP:
                wrong = json.loads(json.dumps(valid))
                wrong[alias]["remote"] = "tor-primary-ro:bucket"
                with self.subTest(path=path, case="same_alias_as_primary"):
                    self.assertFalse(check(wrong))
                wrong = json.loads(json.dumps(valid))
                wrong["tor-primary-ro"]["type"] = "local"
                with self.subTest(path=path, case="primary_is_local"):
                    self.assertFalse(check(wrong))

    def test_empty_s3_source_cannot_look_like_success(self):
        data = BACKUP.read_text(encoding="utf-8")
        self.assertIn('source_inventory=$(rclone --config', data)
        self.assertIn('"$TOR_DR_PRIMARY_REMOTE" --json', data)
        anchor = "printf '%s' \"$source_inventory\" | python3 -c '"
        self.assertIn(anchor, data)
        validator = data.split(anchor, 1)[1].split("' ||", 1)[0]
        samples = (
            ('{"count":1,"bytes":20}', True),
            ('{"count":0,"bytes":0}', False),
            ('{"count":0,"bytes":20}', False),
            ('{"count":"1","bytes":20}', False),
            ('{"count":1,"bytes":0}', False),
            ('{"count":-1,"bytes":20}', False),
            ('{}', False),
            ('not json', False),
        )
        for payload, expected in samples:
            with self.subTest(payload=payload):
                result = subprocess.run(
                    ["python3", "-c", validator],
                    input=payload, text=True, capture_output=True)
                self.assertEqual(result.returncode == 0, expected)

    def test_restore_is_read_only_and_checks_offline_proof(self):
        data = RESTORE.read_text(encoding="utf-8")
        for token in ("[[ ! -v GITHUB_ACTIONS ]]", "tor-offsite-read:",
                      "TOR_DR_AGE_IDENTITY", "TOR_DR_EXPECTED_OBJECT_SHA256",
                      "TOR_DR_EXPECTED_SECRET_SHA256", "age -d -i",
                      "private/", "sha256sum", "config dump",
                      'x.get("type") == "crypt"', "stat -f -c",
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
