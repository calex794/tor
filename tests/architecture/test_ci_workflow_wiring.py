"""Prevent a future path filter or failed planner from silently dropping a suite."""
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT/"tools"))
from ci_test_map import MANUAL_WORKFLOWS, PERFORMANCE_WORKFLOWS, SUITES, TEST_SUITES, workflow_gate


def problems(text, suite):
    result = []
    events = text.split("\npermissions:", 1)[0]
    for event in ("pull_request:", "push:", "workflow_dispatch:"):
        if f"  {event}" not in events:
            result.append(f"missing {event}")
    if re.search(r"^\s+(paths|paths-ignore):", events, re.M):
        result.append("event-level filter can suppress required checks")
    if "    uses: ./.github/workflows/ci-selection.yml\n" not in text:
        result.append("missing reusable planner")
    if f"      caller: {suite}\n" not in text:
        result.append("wrong suite artifact name")
    if "    needs: select\n    if: " + workflow_gate(suite) + "\n" not in text:
        result.append("missing full-run fallback or suite gate")
    return result


class WorkflowWiringTests(unittest.TestCase):
    def test_all_suite_workflows_are_wired_without_path_filters(self):
        for suite, (filename, _) in SUITES.items():
            with self.subTest(suite=suite):
                text = (ROOT/".github/workflows"/filename).read_text()
                self.assertEqual(problems(text, suite), [])

    def test_pr_only_concurrency_preserves_main_and_manual_runs(self):
        expected = (
            "concurrency:\n"
            "  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.run_id }}\n"
            "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n"
        )
        for filename in {name for name, _ in SUITES.values()} | {"rules.yml"}:
            with self.subTest(workflow=filename):
                text = (ROOT/".github/workflows"/filename).read_text()
                self.assertIn(expected, text)
                self.assertEqual(text.count("\nconcurrency:\n"), 1)

    def test_removed_failure_fallback_is_rejected(self):
        text = (ROOT/".github/workflows/storage-crypto.yml").read_text()
        changed = text.replace("needs.select.result != 'success' || ", "")
        self.assertTrue(problems(changed, "crypto"))

    def test_old_event_filter_is_rejected(self):
        text = (ROOT/".github/workflows/tor-reader-visual.yml").read_text()
        changed = text.replace("  pull_request:\n", "  pull_request:\n    paths: ['crates/reading/**']\n")
        self.assertTrue(problems(changed, "reader_browser"))

    def test_rules_are_unconditional_and_deployment_is_separate(self):
        rules = (ROOT/".github/workflows/rules.yml").read_text()
        self.assertNotIn("needs: select", rules)
        self.assertIn("python3 -m unittest discover -s tests/architecture -v", rules)
        for filename in MANUAL_WORKFLOWS:
            with self.subTest(workflow=filename):
                deploy = (ROOT/".github/workflows"/filename).read_text()
                self.assertNotIn("ci-selection", deploy)
                self.assertNotIn("  push:", deploy)
                self.assertNotIn("  pull_request:", deploy)
                self.assertNotIn("  workflow_run:", deploy)
                self.assertIn("  workflow_dispatch:", deploy)

    def test_isolated_performance_workflow_does_not_contact_live_hosts(self):
        for filename in PERFORMANCE_WORKFLOWS:
            with self.subTest(workflow=filename):
                contents = (ROOT/".github/workflows"/filename).read_text()
                self.assertIn("  workflow_dispatch:", contents)
                self.assertIn("  pull_request:", contents)
                self.assertIn("    paths:", contents)
                self.assertNotIn("  push:", contents)
                self.assertNotIn("  pull_request_target:", contents)
                self.assertNotIn("secrets.", contents)
                self.assertIn("127.0.0.1:3089", contents)
                self.assertIn("postgres:18", contents)
                self.assertNotIn("tor-one-click-release", contents)
        harness = (ROOT/"tests/load/tor_http.js").read_text()
        self.assertIn("http://127.0.0.1:3089", harness)
        self.assertNotIn("__ENV.BASE_URL", harness)

    def test_reusable_planner_uses_complete_history_and_no_secrets(self):
        text = (ROOT/".github/workflows/ci-selection.yml").read_text()
        self.assertIn("fetch-depth: 0", text)
        self.assertIn("persist-credentials: false", text)
        self.assertIn("contents: read", text)
        self.assertNotIn("secrets.", text)
        self.assertNotIn("pull_request_target", text)
        self.assertIn("python3 tools/select_feature_tests.py", text)

    def test_no_unknown_automatic_suite_or_missing_harness_in_registry(self):
        known = {filename for filename, _ in SUITES.values()} | {"rules.yml", "ci-selection.yml"} | MANUAL_WORKFLOWS | PERFORMANCE_WORKFLOWS
        self.assertEqual({p.name for p in (ROOT/".github/workflows").glob("*.yml")}, known)
        for path, suites in TEST_SUITES.items():
            with self.subTest(path=path):
                self.assertTrue((ROOT/path).is_file())
                self.assertTrue(suites <= SUITES.keys())


if __name__ == "__main__":
    unittest.main()
