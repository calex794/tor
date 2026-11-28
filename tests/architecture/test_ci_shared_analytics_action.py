"""REUSE-13A: protect actual local action calls and fail-closed shell execution."""
from pathlib import Path
import unittest

from ci_workflow_sources import (
    ACTION_CALL, ACTION_PATH, ACTION_REF, SHELL, STEP_NAMES,
    analytics_steps, expand_analytics,
)

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"


class SharedAnalyticsActionTests(unittest.TestCase):
    def test_actual_callers_keep_check_names_services_and_final_checkout_check(self):
        for filename in ("tor-integration.yml", "tor-pg-reading-analytics.yml"):
            with self.subTest(filename=filename):
                text = (WORKFLOWS / filename).read_text()
                self.assertEqual(text.count(ACTION_CALL), 1)
                job = text.split("\n  pg-reading-analytics:\n", 1)[1]
                import re
                job = re.split(r"\n  [a-z][a-z0-9-]+:\n", job, maxsplit=1)[0]
                self.assertEqual(job.count(ACTION_CALL), 1)
                self.assertIn("    runs-on: ubuntu-24.04\n", job)
                self.assertIn("    timeout-minutes: 35\n", job)
                self.assertIn("image: postgres:18", job)
                self.assertLess(job.index("uses: actions/checkout@v4"), job.index(ACTION_CALL))
                self.assertLess(job.index("uses: Swatinem/rust-cache@v2"), job.index(ACTION_CALL))
                self.assertIn("shared-key: postgres-runtime-no-debug-v1", job)
                self.assertIn("persist-credentials: false", job)
                self.assertLess(job.index(ACTION_CALL), job.index("Confirm committed source was tested"))
                self.assertIn("        if: always()\n        run: git diff --exit-code", job)
                self.assertNotIn("continue-on-error:", job)
                for name in STEP_NAMES:
                    self.assertNotIn("      - name: " + name + "\n", job)
        action = ACTION_PATH.read_text()
        expanded = analytics_steps(action)
        self.assertEqual(action.count(SHELL), len(STEP_NAMES))
        self.assertEqual(expanded.count("      - name: "), len(STEP_NAMES))
        self.assertNotIn("continue-on-error:", action)
        self.assertNotIn("secrets.", action)
        self.assertIn("CI_PG_ANALYTICS_TEST_SECONDS", expanded)

    def test_expansion_rejects_invalid_shell_conditional_and_duplicate_call(self):
        action = ACTION_PATH.read_text()
        for changed in (
            action.replace(SHELL, "      shell: bash\n", 1),
            action.replace(SHELL, SHELL + "      continue-on-error: true\n", 1),
            action.replace(SHELL, SHELL + "      if: false\n", 1),
            action.replace("  using: composite", "  using: node20", 1),
            action.replace("    - name: " + STEP_NAMES[-1],
                           "    - name: Disabled acceptance", 1),
        ):
            with self.subTest(changed=changed[:100]):
                with self.assertRaises(ValueError):
                    expand_analytics(ACTION_CALL, changed)
        for invalid in (ACTION_CALL + ACTION_CALL, ACTION_REF):
            with self.assertRaises(ValueError):
                expand_analytics(invalid, action)

    def test_only_actual_caller_expands(self):
        action = ACTION_PATH.read_text()
        prefix = "    needs: [select, preflight]\n"
        suffix = "      - name: Confirm committed source was tested\n"
        self.assertEqual(expand_analytics(prefix + ACTION_CALL + suffix, action),
                         prefix + analytics_steps(action) + suffix)
        self.assertEqual(expand_analytics(prefix + suffix, action), prefix + suffix)


if __name__ == "__main__":
    unittest.main()
