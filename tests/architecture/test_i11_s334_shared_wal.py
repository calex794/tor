"""RECOVERY-10e-c1: preserve exact S334 evidence through the shared WAL drill."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class SharedS334Wal(unittest.TestCase):
    def test_seed_and_ciphertext_recovery_precede_common_dump(self):
        path = ROOT / "tools/tor_dr_s3_backup_smoke.sh"
        subprocess.run(["bash", "-n", str(path)], check=True)
        shell = path.read_text()
        self.assertIn("s3_34_seed_paid_v1_and_publish_v2", shell)
        self.assertIn("export RUNNER_TEMP=\"$work\"", shell)
        self.assertLess(shell.index("\ns334_seed_and_archive\n"),
                        shell.index('source_finance="$(finance_snapshot'))
        self.assertLess(shell.index('source_finance="$(finance_snapshot'),
                        shell.index("pg_dump --host="))
        for marker in ("openssl rand -out", "s334-content-kek-v2.bin",
                       "sha256_ciphertext", "If-None-Match: *",
                       "== 403", "== 404", 'rm -- "$work/s334-source.enc"',
                       's3 --noproxy \'*\' --output "$work/s334-$edition.enc"',
                       "I11_S334_SHARED_CIPHERTEXT_ARCHIVE_OK"):
            self.assertIn(marker, shell)
        self.assertNotRegex(shell, r'--upload-file[^\n]*(kek|state\.json)')
        self.assertIn("-f tools/i11_s334_paid_recovery_facts.sql", shell)

    def test_composite_witness_runs_in_all_four_physical_snapshots(self):
        shell = (ROOT / "tools/i11_pitr_container.sh").read_text()
        launcher = (ROOT / "tools/i11_pitr_drill.py").read_text()
        host = (ROOT / "tools/tor_dr_s3_backup_smoke.sh").read_text()
        self.assertEqual(shell.count("-f /opt/tor-i11-tools/i11_s334_paid_recovery_facts.sql"), 4)
        self.assertIn("'i11_s334_paid_recovery_facts.sql'", launcher)
        self.assertIn('"$restored_finance" == "$source_finance"', shell)
        self.assertIn('"$app_finance" == "$source_finance"', shell)
        self.assertIn('"$TOR_I11_FINANCE_SHA256"', shell)
        self.assertLess(host.index("python3 tools/i11_pitr_drill.py --fixture-dump"),
                        host.index("I11_S334_SHARED_WAL_SNAPSHOT_OK"))
        self.assertIn("'schema_version': 7", shell)


if __name__ == "__main__":
    unittest.main()
