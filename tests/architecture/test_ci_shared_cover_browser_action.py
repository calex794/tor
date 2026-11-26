"""REUSE-13C: preserve owner browser, MinIO, mailbox and diagnostic boundaries."""
from pathlib import Path
import re
import unittest

from ci_workflow_sources import (
    COVER_ACTION_CALL, COVER_ACTION_PATH, COVER_ACTION_REF,
    COVER_STEP_NAMES, SHELL, cover_browser_steps, expand_cover_browser,
)

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"


class SharedCoverBrowserActionTests(unittest.TestCase):
    def test_real_callers_keep_job_services_setup_and_diagnostics(self):
        for filename in ("tor-integration.yml", "tor-cover-browser.yml"):
            with self.subTest(filename=filename):
                text = (WORKFLOWS / filename).read_text()
                self.assertEqual(text.count(COVER_ACTION_CALL), 1)
                job = re.split(r"\n  [a-z][a-z0-9-]+:\n",
                               text.split("\n  author-cover:\n", 1)[1],
                               maxsplit=1)[0]
                self.assertEqual(job.count(COVER_ACTION_CALL), 1)
                for required in (
                    "    runs-on: ubuntu-24.04", "    timeout-minutes: 40",
                    "image: postgres:18", "persist-credentials: false",
                    "node-version: '22'", "PGHOST: 127.0.0.1",
                    "TOR_TEST_S3_ENDPOINT: http://127.0.0.1:9000",
                ):
                    self.assertIn(required, job)
                for before in ("uses: actions/checkout@v4", "bash tools/ci_prepare_ubuntu_mirror.sh",
                               "uses: dtolnay/rust-toolchain@stable", "uses: Swatinem/rust-cache@v2",
                               "uses: actions/setup-node@v4"):
                    self.assertLess(job.index(before), job.index(COVER_ACTION_CALL))
                self.assertLess(job.index(COVER_ACTION_CALL),
                                job.index("Diagnostic logs if the browser journey fails"))
                self.assertIn("        if: failure()\n        run: |", job)
                self.assertIn("        if: always()\n        uses: actions/upload-artifact@v4", job)
                self.assertIn("name: tor-owner-cover-real-pg-s3-390-1440", job)
                self.assertIn("path: ${{ runner.temp }}/cover-browser-shots/*.png", job)
                self.assertIn("retention-days: 14", job)
                self.assertNotIn("continue-on-error:", job)
                for name in COVER_STEP_NAMES:
                    self.assertNotIn("      - name: " + name + "\n", job)

    def test_preserved_private_storage_and_browser_assertions(self):
        action = COVER_ACTION_PATH.read_text()
        expanded = cover_browser_steps(action)
        self.assertEqual(action.count(SHELL), len(COVER_STEP_NAMES))
        self.assertEqual(expanded.count("      - name: "), len(COVER_STEP_NAMES))
        for required in (
            "bash tools/ci_prepare_pg_client.sh",
            "REVOKE ALL ON DATABASE tor_ci FROM PUBLIC;",
            "DATABASE_URL=\"$MIGRATION_URL\" cargo run -p tor-migrations --bin tor-migrate",
            "RELEASE.2025-09-07T16-13-09Z", 'sha256sum "$RUNNER_TEMP/$release"',
            "--address 127.0.0.1:9000 --console-address 127.0.0.1:9001",
            "http://127.0.0.1:9000/minio/health/ready",
            "if not recipient.endswith('@example.invalid'):",
            "0o600", "cargo test -p tor-ingest --features s3 --test s3_provider",
            "cargo build -p tor-web --features s3-covers",
            "bash tools/ci_prepare_chromium.sh install-and-probe",
            "TOR_COVER_S3_BUCKET: tor-private-cover-ci",
            "DATABASE_URL: ${{ env.APP_URL }}",
            "TOR_PUBLIC_ORIGIN: http://127.0.0.1:3098",
            "AWS_EC2_METADATA_DISABLED: 'true'",
            "TOR_RESET_MAIL_FROM: no-reply@example.invalid",
            "node tools/author_cover_pg_visual_smoke.cjs",
            "node tools/account_line_browser.cjs",
            'test "$code" = 403',
        ):
            with self.subTest(required=required):
                self.assertIn(required, expanded)
        self.assertNotIn("secrets.", action)
        self.assertNotIn("continue-on-error:", action)
        self.assertEqual(expand_cover_browser(COVER_ACTION_CALL, action), expanded)

    def test_rejects_changed_shell_or_browser_environment(self):
        action = COVER_ACTION_PATH.read_text()
        for changed in (
            action.replace(SHELL, "      shell: bash\n", 1),
            action.replace(SHELL, SHELL + "      if: false\n", 1),
            action.replace("DATABASE_URL: ${{ env.APP_URL }}", "DATABASE_URL: external", 1),
            action.replace("TOR_PASSWORD_RESET_ENABLED: '1'", "TOR_PASSWORD_RESET_ENABLED: '0'", 1),
            action.replace("AWS_EC2_METADATA_DISABLED: 'true'", "AWS_EC2_METADATA_DISABLED: 'false'", 1),
        ):
            with self.assertRaises(ValueError):
                cover_browser_steps(changed)
        for invalid in (COVER_ACTION_CALL + COVER_ACTION_CALL, COVER_ACTION_REF):
            with self.assertRaises(ValueError):
                expand_cover_browser(invalid, action)


if __name__ == "__main__":
    unittest.main()
