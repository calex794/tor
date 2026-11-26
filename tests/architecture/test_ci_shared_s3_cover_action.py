"""REUSE-13D: keep real S3 acceptance and the production startup gate distinct."""
from pathlib import Path
import re
import unittest

from ci_workflow_sources import (
    S3_COVER_ACTION_CALL, S3_COVER_ACTION_PATH, S3_COVER_ACTION_REF,
    S3_COVER_STEP_NAMES, SHELL, expand_s3_cover, s3_cover_steps,
)

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"


class SharedS3CoverActionTests(unittest.TestCase):
    def test_actual_callers_keep_job_identity_and_production_proof(self):
        for filename in ("tor-integration.yml", "tor-cover-s3.yml"):
            with self.subTest(filename=filename):
                text = (WORKFLOWS / filename).read_text()
                self.assertEqual(text.count(S3_COVER_ACTION_CALL), 1)
                job = re.split(r"\n  [a-z][a-z0-9-]+:\n",
                               text.split("\n  s3-cover-build:\n", 1)[1],
                               maxsplit=1)[0]
                for required in (
                    "    runs-on: ubuntu-24.04", "    timeout-minutes: 55",
                    "image: postgres:18", "persist-credentials: false",
                    "PGHOST: 127.0.0.1", "TEST_DATABASE_URL:",
                    "MIGRATOR_URL:",
                ):
                    self.assertIn(required, job)
                self.assertEqual(job.count(S3_COVER_ACTION_CALL), 1)
                for before in ("uses: actions/checkout@v4", "uses: dtolnay/rust-toolchain@stable",
                               "uses: Swatinem/rust-cache@v2"):
                    self.assertLess(job.index(before), job.index(S3_COVER_ACTION_CALL))
                lint = "      - name: Combined S3 feature Clippy (paired Rust and PostgreSQL)\n"
                production = ("      - name: Build the exact production profile and prove actual startup with S3 configured\n"
                              "        run: |\n"
                              "          bash tools/build_tor_production_web.sh\n"
                              "          bash tools/test_tor_production_web_startup.sh\n")
                self.assertIn(production, job)
                self.assertLess(job.index(S3_COVER_ACTION_CALL), job.index(lint))
                self.assertLess(job.index(lint), job.index(production))
                self.assertLess(job.index(production), job.index("MinIO diagnostics on failure"))
                self.assertIn("        if: failure()\n        run: tail -n 80", job)
                self.assertNotIn("continue-on-error:", job)
                for name in S3_COVER_STEP_NAMES:
                    self.assertNotIn("      - name: " + name + "\n", job)

    def test_private_storage_readonly_and_integrity_checks_are_preserved(self):
        action = S3_COVER_ACTION_PATH.read_text()
        expanded = s3_cover_steps(action)
        self.assertEqual(action.count(SHELL), len(S3_COVER_STEP_NAMES))
        self.assertEqual(expanded.count("      - name: "), len(S3_COVER_STEP_NAMES))
        for required in (
            "cargo check -p tor-web --features s3-covers",
            "RELEASE.2025-09-07T16-13-09Z", 'test "${#expected}" -eq 64',
            'test "$actual" = "$expected"',
            "--address 127.0.0.1:9000 --console-address 127.0.0.1:9001",
            "http://127.0.0.1:9000/minio/health/ready",
            "cargo test -p tor-ingest --features s3 --test s3_provider",
            "s023_private_source_put_get_conflict_and_delete",
            "bash tools/ci_prepare_pg_client.sh",
            "REVOKE ALL ON DATABASE tor_ci FROM PUBLIC;",
            'DATABASE_URL="$MIGRATOR_URL" cargo run -p tor-migrations --bin tor-migrate',
            "obs09-owner@example.invalid",
            "cargo test -p tor-web --features s3-covers,test-cdn-loopback --test cover_s3_pg",
            "tools/mod_cover_gc_dry_run.sql", "private_variants", "private_sources",
            "retain_attached_slot", "tools/cover_lossy_inventory.sql",
            "tor-cover-webp-lossy-q85-v2", "JOIN book_cover_slots",
            "j.book_id=950", "j.status='done'", "v.variant_name='card'",
            'test "$status" = 403', 'test "$code" = 403',
        ):
            with self.subTest(required=required):
                self.assertIn(required, expanded)
        self.assertEqual(expanded.count("grep -Fq 'ROLLBACK'"), 2)
        for forbidden in ("secrets.", "continue-on-error:", "needs.select",
                          "build_tor_production_web.sh", "test_tor_production_web_startup.sh"):
            self.assertNotIn(forbidden, action)
        self.assertEqual(expand_s3_cover(S3_COVER_ACTION_CALL, action), expanded)

    def test_changed_shell_or_privileged_environment_is_rejected(self):
        action = S3_COVER_ACTION_PATH.read_text()
        for changed in (
            action.replace(SHELL, "      shell: bash\n", 1),
            action.replace(SHELL, SHELL + "      if: false\n", 1),
            action.replace("PGUSER: tor_app", "PGUSER: postgres", 1),
            action.replace("PGDATABASE: tor_ci", "PGDATABASE: postgres", 1),
            action.replace("TOR_TEST_S3_ENDPOINT: http://127.0.0.1:9000",
                           "TOR_TEST_S3_ENDPOINT: https://external.invalid", 1),
        ):
            with self.assertRaises(ValueError):
                s3_cover_steps(changed)
        for invalid in (S3_COVER_ACTION_CALL + S3_COVER_ACTION_CALL, S3_COVER_ACTION_REF):
            with self.assertRaises(ValueError):
                expand_s3_cover(invalid, action)


if __name__ == "__main__":
    unittest.main()
