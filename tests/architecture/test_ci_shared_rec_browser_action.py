"""REUSE-13B: keep both REC lanes, job identity and diagnostic artifacts."""
from pathlib import Path
import re
import subprocess
import unittest

from ci_workflow_sources import (
    REC_ACTION_CALL, REC_ACTION_PATH, REC_ACTION_REF, REC_LANE_GUARD,
    REC_STEP_NAMES, SHELL, expand_rec_browser, rec_browser_steps,
)

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"


class SharedRecBrowserActionTests(unittest.TestCase):
    def test_real_callers_keep_matrix_artifacts_and_job_boundaries(self):
        for filename in ("tor-integration.yml", "tor-pg-rec-browser.yml"):
            with self.subTest(filename=filename):
                text = (WORKFLOWS / filename).read_text()
                self.assertEqual(text.count(REC_ACTION_CALL), 1)
                job = re.split(r"\n  [a-z][a-z0-9-]+:\n",
                               text.split("\n  pg-rec-browser:\n", 1)[1],
                               maxsplit=1)[0]
                self.assertEqual(job.count(REC_ACTION_CALL), 1)
                for required in (
                    "    runs-on: ubuntu-24.04", "    timeout-minutes: 35",
                    "lane: [current, historical]", "fail-fast: false",
                    "image: postgres:18", "persist-credentials: false",
                    "shared-key: postgres-runtime-no-debug-v1", "node-version: '22'",
                ):
                    self.assertIn(required, job)
                self.assertLess(job.index("uses: actions/checkout@v4"), job.index(REC_ACTION_CALL))
                self.assertLess(job.index("uses: actions/setup-node@v4"), job.index(REC_ACTION_CALL))
                self.assertLess(job.index(REC_ACTION_CALL), job.index("Confirm committed source was tested"))
                self.assertIn("        if: always()\n        run: git diff --exit-code", job)
                self.assertIn("      - uses: actions/upload-artifact@v4\n        if: always()", job)
                self.assertIn("name: pg-rec-browser-${{ github.sha }}-${{ matrix.lane }}", job)
                self.assertIn("${{ runner.temp }}/rec13-browser/*.log", job)
                self.assertIn("${{ runner.temp }}/rec13-browser/shots/*.png", job)
                self.assertIn("retention-days: 3", job)
                self.assertNotIn("continue-on-error:", job)
                for name in REC_STEP_NAMES:
                    self.assertNotIn("      - name: " + name + "\n", job)
        specialist = (WORKFLOWS / "rec-feedback-acceptance.yml").read_text()
        self.assertNotIn(REC_ACTION_REF, specialist)
        self.assertNotIn("TOR_REC13_LANE:", specialist)
        self.assertIn("run: bash tools/rec13_browser_acceptance.sh", specialist)

    def test_exact_lane_conditions_and_original_steps(self):
        action = REC_ACTION_PATH.read_text()
        expanded = rec_browser_steps(action)
        self.assertEqual(expanded.count("      - name: "), len(REC_STEP_NAMES))
        self.assertEqual(action.count(SHELL), len(REC_STEP_NAMES) + 1)
        self.assertEqual(action.count("if: ${{ inputs.lane == 'historical' }}"), 2)
        self.assertEqual(action.count("if: ${{ inputs.lane == 'current' }}"), 1)
        self.assertIn("TOR_REC13_LANE: ${{ matrix.lane }}", expanded)
        self.assertNotIn("continue-on-error:", action)
        self.assertNotIn("secrets.", action)
        self.assertEqual(expand_rec_browser(REC_ACTION_CALL, action), expanded)

    def test_invalid_lane_and_shell_fail_closed(self):
        action = REC_ACTION_PATH.read_text()
        guard = REC_LANE_GUARD.split("      run: |\n", 1)[1]
        command = "".join(line[8:] for line in guard.splitlines(keepends=True))
        for lane, expected in (("current", 0), ("historical", 0), ("all", 2),
                               ("", 2), ("unknown", 2), ("current; exit 0", 2)):
            with self.subTest(lane=lane):
                result = subprocess.run(["/bin/bash", "-e", "-c", command],
                                        env={"TOR_REC13_LANE": lane},
                                        capture_output=True, text=True, check=False)
                self.assertEqual(result.returncode, expected)
                if expected:
                    self.assertIn("Unsupported REC matrix lane", result.stderr)
        for changed in (
            action.replace(SHELL, "      shell: bash\n", 1),
            action.replace("current|historical)", "all|current|historical)", 1),
            action.replace("if: ${{ inputs.lane == 'historical' }}",
                           "if: ${{ inputs.lane == 'current' }}", 1),
            action.replace("if: ${{ inputs.lane == 'current' }}", "if: false", 1),
            action.replace("      env:\n        TOR_REC13_LANE: ${{ inputs.lane }}\n",
                           "      env:\n        TOR_REC13_LANE: all\n", 1),
        ):
            with self.assertRaises(ValueError):
                rec_browser_steps(changed)
        for invalid in (REC_ACTION_CALL + REC_ACTION_CALL, REC_ACTION_REF,
                        REC_ACTION_CALL.replace("${{ matrix.lane }}", "all")):
            with self.assertRaises(ValueError):
                expand_rec_browser(invalid, action)


if __name__ == "__main__":
    unittest.main()
