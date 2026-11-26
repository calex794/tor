"""REUSE-13E-a: keep exact PG shard dispatch, timing and fail-fast behavior."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

from ci_workflow_sources import (
    PG_RUNTIME_CALL, PG_RUNTIME_PATH, PG_RUNTIME_PREFIX, PG_RUNTIME_REF,
    expand_pg_runtime,
)

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"
FAKE_CARGO = """#!/bin/bash
printf '%s\\t%s\\t%s\\t%s\\n' "$*" "${TOR_PAID_CHECKOUT_ENABLED-unset}" "${TOR_DIRECT_SWAP_BROWSER_ENABLED-unset}" "${TOR_RIL_HMAC_KEY-unset}" >> "$TRACE"
if [[ -n "$FAIL_MATCH" && "$*" == *"$FAIL_MATCH"* ]]; then
  exit 17
fi
"""


class SharedPgRuntimeAcceptanceTests(unittest.TestCase):
    def test_real_callers_keep_shard_job_and_surrounding_steps(self):
        for filename in ("tor-integration.yml", "tor-pg-runtime.yml"):
            with self.subTest(filename=filename):
                text = (WORKFLOWS / filename).read_text()
                self.assertEqual(text.count(PG_RUNTIME_CALL), 1)
                job = re.split(r"\n  [a-z][a-z0-9-]+:\n",
                               text.split("\n  postgres-runtime:\n", 1)[1],
                               maxsplit=1)[0]
                self.assertEqual(job.count(PG_RUNTIME_CALL), 1)
                self.assertIn("shard: [foundation, reading, commerce]", job)
                self.assertIn("CI_PG_SHARD: ${{ matrix.shard }}", job)
                self.assertIn("image: postgres:18", job)
                self.assertIn("fail-fast: false", job)
                self.assertLess(job.index("Advance explicit-ID fixture sequences as schema owner"),
                                job.index(PG_RUNTIME_CALL))
                self.assertLess(job.index(PG_RUNTIME_CALL),
                                job.index("Disk usage after database and HTTP integration"))
                self.assertLess(job.index(PG_RUNTIME_CALL),
                                job.index("D-04 real HTTP and PostgreSQL sessions"))
                self.assertNotIn("ci_pg_phase_done ", job)
                self.assertNotIn("continue-on-error:", job)
        subprocess.run(["/bin/bash", "-n", str(PG_RUNTIME_PATH)], check=True)

    def invoke(self, shard, fail=""):
        # No actual Cargo, PostgreSQL, browser, S3 or network calls.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stub = root / "cargo"
            stub.write_text(FAKE_CARGO)
            stub.chmod(0o700)
            trace = root / "trace"
            result = subprocess.run(
                ["/bin/bash", str(PG_RUNTIME_PATH)], cwd=ROOT,
                env={"PATH": tmp + ":/usr/bin:/bin", "TRACE": str(trace),
                     "CI_PG_SHARD": shard, "FAIL_MATCH": fail,
                     "TOR_PAID_CHECKOUT_ENABLED": "sentinel",
                     "TOR_DIRECT_SWAP_BROWSER_ENABLED": "sentinel"},
                capture_output=True, text=True, check=False,
            )
            rows = [line.split("\t") for line in trace.read_text().splitlines()] if trace.exists() else []
            return result, rows

    def test_all_three_shards_keep_counts_order_timing_and_feature_flags(self):
        cases = (
            ("foundation", 60, 60, "ril_rollups_pg", "paid_assets_pg",
             ("traffic_storage_catalog", "books_import_authoring")),
            ("reading", 50, 50, "postgres_reading", "text_issues_pg",
             ("reading_library", "referrals_recommendations_social")),
            ("commerce", 59, 54, "slices_pg d_02_", "ril_behavior_consent_pg",
             ("commerce_support", "http_security")),
        )
        for shard, count, timed, first, last, phases in cases:
            with self.subTest(shard=shard):
                result, rows = self.invoke(shard)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(rows), count)
                self.assertTrue(all(len(row) == 4 for row in rows))
                self.assertIn(first, rows[0][0])
                self.assertIn(last, rows[-1][0])
                self.assertEqual(tuple(re.findall(r"CI_PG_PHASE_SECONDS phase=(\w+)", result.stdout)), phases)
                timing = re.findall(r"CI_PG_CARGO_MS seq=(\d+) .* status=(\d+)", result.stdout)
                self.assertEqual(timing, [(str(n), "0") for n in range(1, timed + 1)])
                if shard == "commerce":
                    disabled = next(row for row in rows if "d_06_swap_http_disabled_by_default" in row[0])
                    enabled = next(row for row in rows if "d_06_swap_http_preview_csrf_actor" in row[0])
                    self.assertEqual(disabled[1:3], ["unset", "unset"])
                    self.assertEqual(enabled[1:3], ["1", "1"])
                    self.assertEqual(rows[-1][3], "4f" * 32)

    def test_failed_command_stops_each_path_and_invalid_shard_runs_nothing(self):
        for shard, match in (("foundation", "ril_rollups_pg"),
                             ("reading", "postgres_reading"),
                             ("commerce", "slices_pg d_02_"),
                             ("commerce", "d_06_swap_http_disabled_by_default")):
            with self.subTest(shard=shard, match=match):
                result, rows = self.invoke(shard, match)
                self.assertEqual(result.returncode, 17)
                self.assertIn(match, rows[-1][0])
                self.assertNotIn("CI_PG_PHASE_SECONDS phase=http_security", result.stdout)
                if "disabled_by_default" not in match:
                    self.assertEqual(len(rows), 1)
                    self.assertIn("status=17", result.stdout)
        for shard in ("", "all", "unknown"):
            result, rows = self.invoke(shard)
            self.assertEqual(result.returncode, 2)
            self.assertEqual(rows, [])
            self.assertIn("Unsupported PostgreSQL CI shard", result.stderr)

    def test_expansion_rejects_invalid_guard_and_ambiguous_call(self):
        script = PG_RUNTIME_PATH.read_text()
        self.assertTrue(script.startswith(PG_RUNTIME_PREFIX))
        self.assertIn('command cargo "$@" || status=$?', script)
        for changed in (script.replace("set -e\n", "set +e\n", 1),
                        script.replace("foundation|reading|commerce)", "all|foundation|reading|commerce)", 1)):
            with self.assertRaises(ValueError):
                expand_pg_runtime(PG_RUNTIME_CALL, changed)
        for invalid in (PG_RUNTIME_CALL + PG_RUNTIME_CALL, PG_RUNTIME_REF):
            with self.assertRaises(ValueError):
                expand_pg_runtime(invalid, script)


if __name__ == "__main__":
    unittest.main()
