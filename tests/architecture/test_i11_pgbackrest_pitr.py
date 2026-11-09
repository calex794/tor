"""DR-02: templates are operator-only and never mutate production in CI."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
OPS = ROOT / "ops"


class PgBackRestDr02Tests(unittest.TestCase):
    def test_readonly_preflight_syntax(self):
        check = OPS / "check-pgbackrest-pitr-readiness.sh"
        result = subprocess.run(["bash", "-n", str(check)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        source = check.read_text()
        for need in ("data_directory", "archive_mode", "archive_command",
                     "archive_timeout", "pgbackrest", "NOT_RESTORED"):
            self.assertIn(need, source)
        for banned in ("ALTER SYSTEM", "pg_ctlcluster", "pgbackrest restore",
                       "pgbackrest stanza-create"):
            self.assertNotIn(banned, source)

    def test_archiving_is_explicit_and_fail_closed(self):
        config = (OPS / "pgbackrest-pitr-postgresql.conf.example").read_text()
        self.assertIn("archive_mode=on", config)
        self.assertIn("archive_timeout=900s", config)
        self.assertIn("archive-push %p", config)
        self.assertNotIn("archive_command='true'", config)

    def test_s3_configuration_has_no_real_keys(self):
        conf = (OPS / "pgbackrest-pitr.conf.example").read_text()
        self.assertIn("repo1-type=s3", conf)
        self.assertIn("repo1-cipher-type=aes-256-cbc", conf)
        self.assertIn("repo1-retention-full=4", conf)
        self.assertIn("REPLACE_WITH_", conf)
        self.assertNotIn("repo1-cipher-pass=000000", conf)

    def test_full_and_diff_timer_are_separate_and_no_auto_activation(self):
        full = (OPS / "tor-pgbackrest-full.timer.example").read_text()
        diff = (OPS / "tor-pgbackrest-diff.timer.example").read_text()
        self.assertIn("OnCalendar=Sun", full)
        self.assertIn("OnCalendar=Mon..Sat", diff)
        for path in ("tor-pgbackrest-full.service.example",
                     "tor-pgbackrest-diff.service.example"):
            body = (OPS / path).read_text()
            self.assertIn("User=postgres", body)
            self.assertIn("--stanza=tor", body)
        for path in ("tor-pgbackrest-full.timer.example",
                     "tor-pgbackrest-diff.timer.example"):
            self.assertNotIn("WantedBy=multi-user.target", (OPS / path).read_text())


if __name__ == "__main__":
    unittest.main()
