"""Safety boundary for the manually selected S3-enabled TOR candidate build."""

from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = (ROOT / ".github/workflows/tor-s3-candidate-build.yml").read_text()
PRODUCTION = (ROOT / ".github/workflows/tor-one-click-release.yml").read_text()
RUNBOOK = (ROOT / "docs/infra/tor-s3-existing-tore-activation.md").read_text()


class S3CandidateBuildTests(unittest.TestCase):
    def test_manual_dispatch_only_with_read_only_git(self):
        events = WORKFLOW.split("\npermissions:", 1)[0] + "\n"
        self.assertIn("  workflow_dispatch:\n", events)
        for forbidden in ("  pull_request:", "  pull_request_target:", "  push:",
                          "  workflow_run:", "  schedule:"):
            self.assertNotIn(forbidden, events)
        self.assertIn("  contents: read", WORKFLOW)
        self.assertIn("persist-credentials: false", WORKFLOW)
        self.assertNotIn("  environment:", WORKFLOW)
        self.assertNotIn("contents: write", WORKFLOW)

    def test_no_deployment_credentials_commands_or_provider_calls(self):
        for forbidden in ("secrets.", "vars.", "TOR_DEPLOY_", "AWS_ACCESS_KEY",
                          "AWS_SECRET", "TOR_FND02_YANDEX_", "TOR_FND07_YANDEX_",
                          "sudo ", "ssh ", "scp ", "systemctl", "/etc/tor/",
                          "storage.yandexcloud.net", "aws s3", "kubectl ",
                          "go-live-postgres", "tor-one-click-release.yml"):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, WORKFLOW)

    def test_exact_free_s3_build_profile_and_evidence_not_binary(self):
        self.assertIn(
            'cargo build --release --locked -p tor-web --features "s3-free,s3-chapter-import,s3-covers"',
            WORKFLOW,
        )
        self.assertNotIn('"s3-free,s3-chapter-import,s3-covers,s3-paid"', WORKFLOW)
        for item in ("TOR_PROVIDER_ACCEPTED=NO", "TOR_DATA_READY=NO",
                     "TOR_DEPLOYMENT=NONE", "TOR_S3_PAID=OFF"):
            self.assertIn(item, WORKFLOW)
        upload = WORKFLOW.split("uses: actions/upload-artifact@v4", 1)[1]
        self.assertIn("tor-s3-build.sha256", upload)
        self.assertIn("tor-s3-build-evidence.txt", upload)
        self.assertNotIn("target/release/", upload)
        self.assertNotIn("tor_id_ed25519", upload)

    def test_production_release_is_manual_and_uses_reviewed_complete_profile(self):
        self.assertIn("environment: tor-staging", PRODUCTION)
        self.assertIn("  workflow_dispatch:", PRODUCTION)
        self.assertIn("bash tools/build_tor_production_web.sh", PRODUCTION)
        profile = (ROOT / "tools/build_tor_production_web.sh").read_text()
        self.assertIn("--features 's3-covers,s3-free,s3-paid,s3-chapter-import'", profile)
        self.assertIn("--release-capabilities", profile)
        self.assertNotIn("test-cdn-loopback", profile)
        self.assertNotIn("cargo build --release -p tor-web\n", PRODUCTION)

    def test_existing_tore_is_not_modified_and_publisher_is_separate(self):
        for item in ("existing `tore` bucket", "`tor-s3-publisher`",
                     "storage.uploader", "CopyObject", "If-None-Match",
                     "staging/chapters/", "DELETE/GC stays OFF",
                     "TOR_FREE_S3_BUCKET=tore", "NO-GO", "no binary or secrets"):
            with self.subTest(item=item):
                self.assertIn(item, RUNBOOK)
        self.assertIn("Do not enable S3 bucket versioning", RUNBOOK)


if __name__ == "__main__":
    unittest.main()
