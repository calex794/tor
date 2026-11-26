"""REUSE-13F: exercise isolated shell control flow without real paid fixtures.

These stub commands never call Cargo, Node, PostgreSQL, MinIO, or the real
evidence writer. All mock output is confined to a temporary directory.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

from ci_workflow_sources import PAID_E2E_SCRIPTS, expand_paid_e2e

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"
FIXTURE_ENV = {
    "GITHUB_ACTIONS": "true",
    "PGHOST": "127.0.0.1",
    "PGDATABASE": "postgres",
    "TEST_DATABASE_URL": "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci",
    "MIGRATOR_URL": "postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci",
    "TOR_TEST_S3_ENDPOINT": "http://127.0.0.1:9000",
}
STUB = r'''#!/usr/bin/python3
import json
import os
from pathlib import Path
import sys

name = Path(sys.argv[0]).name
args = sys.argv[1:]
label = name + " " + " ".join(args)
with Path(os.environ["TRACE"]).open("a") as out:
    out.write(json.dumps([name, args, os.environ.get("TOR_PAID_CHECKOUT_ENABLED"),
                          os.environ.get("TOR_LAYER_READER_ENABLED")]) + "\n")
if os.environ["FAIL_MATCH"] and os.environ["FAIL_MATCH"] in label:
    sys.exit(17)
if name == "cargo" and "paid_grants_s3_pg" in args:
    Path(os.environ["TOR_PAID_GRANT_FIXTURE_PATH"]).write_text("test fixture")
if name == "node" and args[0] in (
    "tools/layer_reader_minio_chromium_e2e.cjs",
    "tools/paid_grant_webcrypto_e2e.mjs",
    "tools/paid_reader_minio_chromium_e2e.cjs",
    "tools/paid_reader_private_media_browser.cjs",
):
    assert Path(os.environ["TOR_PAID_GRANT_FIXTURE_PATH"]).is_file()
if name == "cargo" and "free_inventory_operator_pg_minio" in args:
    print("MOCK operator output")
if name == "bash" and args == ["tools/drop_04d_worker_ci.sh"]:
    print("MOCK worker output")
if name == "python3" and args[:3] == [
    "tools/verify_ci_gate.py", "record-foundation", "paid-private-e2e"
]:
    assert Path(args[3]).read_text() == "MOCK operator output\nMOCK worker output\n"
    Path(os.environ["GITHUB_OUTPUT"]).write_text("MOCK_ONLY\n")
'''


class SharedPaidE2EAcceptanceTests(unittest.TestCase):
    def test_callers_keep_real_evidence_outputs_and_original_steps(self):
        for filename in ("tor-integration.yml", "tor-paid-grant-e2e.yml"):
            with self.subTest(filename=filename):
                raw = (WORKFLOWS / filename).read_text()
                job = re.split(r"\n  [a-z][a-z0-9-]+:\n",
                               raw.split("\n  paid-private-e2e:\n", 1)[1],
                               maxsplit=1)[0]
                self.assertIn("foundation_evidence: ${{ steps.foundation.outputs.foundation_evidence }}", job)
                self.assertEqual(job.count("        id: foundation\n"), 1)
                self.assertIn("image: postgres:18", job)
                self.assertIn("    timeout-minutes: 35", job)
                self.assertIn("persist-credentials: false", job)
                for path in PAID_E2E_SCRIPTS:
                    self.assertEqual(job.count(f"        run: bash {path}\n"), 1)
                self.assertLess(job.index(PAID_E2E_SCRIPTS[0]), job.index(PAID_E2E_SCRIPTS[1]))
                self.assertLess(job.index(PAID_E2E_SCRIPTS[1]),
                                job.index("Install real Chromium before issuing short-lived S3 URLs"))
                self.assertLess(job.index("Install real Chromium before issuing short-lived S3 URLs"),
                                job.index("        id: foundation"))
                self.assertLess(job.index("        id: foundation"), job.index(PAID_E2E_SCRIPTS[2]))
                self.assertLess(job.index(PAID_E2E_SCRIPTS[2]), job.index("Save masked real NEwBOO"))
                self.assertIn("        if: always()\n        uses: actions/upload-artifact@v4", job)
                self.assertIn("path: ${{ runner.temp }}/tor-paid-reader-visual/*.png", job)
                self.assertIn("        if: failure()\n        run: tail -n 80", job)
                self.assertNotIn("continue-on-error:", job)
                expanded = expand_paid_e2e(job)
                for marker in (
                    "REVOKE ALL ON DATABASE tor_ci FROM PUBLIC;",
                    'test "$actual" = "$expected"',
                    "MINIO_API_CORS_ALLOW_ORIGIN=http://127.0.0.1:3098",
                    "--address 127.0.0.1:9000 --console-address 127.0.0.1:9001",
                    "cargo test -p tor-web --features s3-paid --test paid_grants_s3_pg",
                    "cargo test -p tor-web --features s3-free,s3-paid --test layer_free_private_s3_pg",
                    'python3 tools/verify_ci_gate.py record-foundation paid-private-e2e "$foundation_log"',
                ):
                    self.assertIn(marker, expanded)
        for path in PAID_E2E_SCRIPTS:
            subprocess.run(["/bin/bash", "-n", str(ROOT / path)], check=True)

    def invoke(self, path, fail="", override=None):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trace, output = root / "trace", root / "mock-output"
            for name in ("cargo", "node", "python3", "bash"):
                stub = root / name
                stub.write_text(STUB)
                stub.chmod(0o700)
            env = dict(FIXTURE_ENV, PATH=tmp + ":/usr/bin:/bin", TRACE=str(trace),
                       RUNNER_TEMP=tmp, GITHUB_OUTPUT=str(output), FAIL_MATCH=fail)
            env.update(override or {})
            result = subprocess.run(
                ["/bin/bash", str(ROOT / path)], cwd=ROOT, env=env,
                capture_output=True, text=True, check=False, timeout=20,
            )
            calls = [json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else []
            receipt = output.read_text() if output.exists() else None
            leftovers = list(root.glob("tor-fnd07.*")) + list(root.glob("paid-grant-e2e.json"))
            return result, calls, receipt, leftovers

    def test_mock_paid_sequence_keeps_short_url_checks_and_cleanup(self):
        result, calls, receipt, leftovers = self.invoke(PAID_E2E_SCRIPTS[2])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(receipt, "MOCK_ONLY\n")
        self.assertEqual(leftovers, [])
        purchase = next(i for i, c in enumerate(calls) if "paid_grants_s3_pg" in c[1])
        self.assertEqual(calls[purchase][2:], ["1", "1"])
        self.assertEqual([c[:2] for c in calls[purchase + 1:purchase + 5]], [
            ["node", ["tools/layer_reader_minio_chromium_e2e.cjs"]],
            ["node", ["tools/paid_grant_webcrypto_e2e.mjs"]],
            ["node", ["tools/paid_reader_minio_chromium_e2e.cjs"]],
            ["node", ["tools/paid_reader_private_media_browser.cjs"]],
        ])
        self.assertEqual(calls[-2][:2], ["bash", ["tools/drop_04d_worker_ci.sh"]])
        self.assertEqual(calls[-1][0], "python3")
        self.assertEqual(calls[-1][1][:3], [
            "tools/verify_ci_gate.py", "record-foundation", "paid-private-e2e"])

    def test_failures_in_cargo_browser_pipes_and_verifier_do_not_emit_receipt(self):
        for fail in ("paid_grants_s3_pg", "node tools/paid_grant_webcrypto_e2e.mjs",
                     "free_inventory_operator_pg_minio", "bash tools/drop_04d_worker_ci.sh",
                     "record-foundation paid-private-e2e"):
            with self.subTest(fail=fail):
                result, calls, receipt, leftovers = self.invoke(PAID_E2E_SCRIPTS[2], fail=fail)
                self.assertEqual(result.returncode, 17, result.stderr)
                self.assertIsNone(receipt)
                self.assertEqual(leftovers, [])
                self.assertIn(fail, calls[-1][0] + " " + " ".join(calls[-1][1]))

    def test_all_scripts_refuse_nonisolated_env_before_any_command(self):
        for path in PAID_E2E_SCRIPTS:
            for override in ({"GITHUB_ACTIONS": "false"}, {"PGHOST": "external.invalid"},
                             {"TEST_DATABASE_URL": "postgresql://external.invalid/production"}):
                with self.subTest(path=path, override=override):
                    result, calls, receipt, leftovers = self.invoke(path, override=override)
                    self.assertEqual(result.returncode, 2)
                    self.assertEqual(calls, [])
                    self.assertIsNone(receipt)
                    self.assertEqual(leftovers, [])
        result, calls, receipt, leftovers = self.invoke(
            PAID_E2E_SCRIPTS[2], override={"TOR_TEST_S3_ENDPOINT": "https://external.invalid"})
        self.assertEqual(result.returncode, 2)
        self.assertEqual(calls, [])
        self.assertIsNone(receipt)
        self.assertEqual(leftovers, [])


if __name__ == "__main__":
    unittest.main()
