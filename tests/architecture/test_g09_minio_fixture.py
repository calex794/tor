"""G-09: both provider jobs must use the same fixed, test-only fixture."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
FIX = "52eee5a2f19ea8ef06bf2dd5ab64ab41424cb591"


class G09MinioFixtureTests(unittest.TestCase):
    def test_exact_upstream_build_and_syntax(self):
        path = ROOT / "tools/g09_build_minio_ci.sh"
        script = path.read_text()
        subprocess.run(["bash", "-n", str(path)], check=True)
        for marker in (
            'test "${GITHUB_ACTIONS:-}" = true',
            "export GOTOOLCHAIN=local",
            "go1.24.8",
            "sha=" + FIX,
            'git fetch --depth=1 origin "$sha"',
            'test "$(git rev-parse HEAD)" = "$sha"',
            "go build -mod=readonly",
            "git diff --exit-code -- go.mod go.sum",
            "sha256sum",
        ):
            self.assertIn(marker, script)

    def test_both_provider_jobs_keep_strict_suite(self):
        for filename in ("storage-s3.yml", "tor-integration.yml"):
            with self.subTest(workflow=filename):
                body = (ROOT / ".github/workflows" / filename).read_text()
                body = body.split("\n  s3-provider:\n", 1)[1]
                body = body.split("\n  s3-cover-build:\n", 1)[0]
                self.assertEqual(body.count("run: bash tools/g09_build_minio_ci.sh"), 1)
                self.assertIn("go-version: '1.24.8'", body)
                self.assertIn("release=minio-g09-fixed", body)
                self.assertNotIn("RELEASE.2025-09-07T16-13-09Z", body)
                self.assertIn("bash tools/g09_minio_tcp_metadata_ci.sh", body)
                self.assertIn("--test free_pack_range_s3 -- --ignored --test-threads=1", body)
                self.assertIn("cargo clippy -p tor-storage --features s3 --all-targets -- -D warnings", body)


if __name__ == "__main__":
    unittest.main()
