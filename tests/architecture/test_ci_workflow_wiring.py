"""Prevent dropped CI suites, duplicate selectors or divergent manual wrappers."""
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import (
    CI_WORKFLOW, MANUAL_WORKFLOWS, PERFORMANCE_WORKFLOWS, SUITES,
    SUITE_JOB_IDS, TEST_SUITES, workflow_gate,
)
WORKFLOWS = ROOT / ".github" / "workflows"


def job_sections(text):
    parts = text.split("\njobs:\n", 1)
    if len(parts) != 2:
        return {}
    body = parts[1]
    entries = list(re.finditer(r"^  ([A-Za-z0-9_-]+):\n", body, re.M))
    return {
        item.group(1): body[item.start():(entries[i + 1].start()
                                     if i + 1 < len(entries) else len(body))]
        for i, item in enumerate(entries)
    }


def problems(text, suite):
    """Standalone suites must be manual-only and preserve their guarded job."""
    result = []
    events = text.split("\npermissions:", 1)[0]
    if "  workflow_dispatch:\n" not in events:
        result.append("missing manual dispatch")
    for forbidden in ("  pull_request:\n", "  push:\n", "  workflow_run:\n"):
        if forbidden in events:
            result.append("manual wrapper starts automatic CI")
    if re.search(r"^\s+(paths|paths-ignore):", events, re.M):
        result.append("event-level filter is forbidden")
    sections = job_sections(text)
    if set(sections) != {"select", SUITE_JOB_IDS[suite]}:
        result.append("manual suite has missing or extra jobs")
    if "    uses: ./.github/workflows/ci-selection.yml\n" not in sections.get("select", ""):
        result.append("missing reusable planner")
    if f"      caller: {suite}\n" not in sections.get("select", ""):
        result.append("wrong suite artifact name")
    if "    needs: select\n    if: " + workflow_gate(suite) + "\n" not in sections.get(SUITE_JOB_IDS[suite], ""):
        result.append("missing full-run fallback or suite gate")
    return result


class WorkflowWiringTests(unittest.TestCase):
    def test_single_automatic_selector_and_complete_suite_jobs(self):
        text = (WORKFLOWS / CI_WORKFLOW).read_text()
        events = text.split("\npermissions:", 1)[0]
        for event in ("pull_request:", "push:", "workflow_dispatch:"):
            self.assertIn(f"  {event}", events)
        self.assertNotRegex(events, r"(?m)^\s+(paths|paths-ignore):")
        sections = job_sections(text)
        self.assertEqual(set(sections), {"select"} | set(SUITE_JOB_IDS.values()))
        self.assertEqual(text.count("    uses: ./.github/workflows/ci-selection.yml\n"), 1)
        self.assertIn("      caller: all\n", sections["select"])
        self.assertNotIn("pull_request_target:", text)
        for suite, (filename, _) in SUITES.items():
            with self.subTest(suite=suite):
                old = (WORKFLOWS / filename).read_text()
                self.assertEqual(problems(old, suite), [])
                job_id = SUITE_JOB_IDS[suite]
                self.assertIn("    needs: select\n    if: " + workflow_gate(suite) + "\n",
                              sections[job_id])
                # Mirror every command, service, artifact and safety gate.
                self.assertEqual(sections[job_id], job_sections(old)[job_id])

    def test_pr_only_concurrency_preserves_main_and_manual_runs(self):
        expected = (
            "concurrency:\n"
            "  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.run_id }}\n"
            "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n"
        )
        for filename in (CI_WORKFLOW, "rules.yml"):
            with self.subTest(workflow=filename):
                text = (WORKFLOWS / filename).read_text()
                self.assertIn(expected, text)
                self.assertEqual(text.count("\nconcurrency:\n"), 1)

    def test_removed_failure_fallback_is_rejected(self):
        old = (WORKFLOWS / "storage-crypto.yml").read_text()
        changed = old.replace("needs.select.result != 'success' || ", "")
        self.assertTrue(problems(changed, "crypto"))
        combined = (WORKFLOWS / CI_WORKFLOW).read_text()
        changed = combined.replace("needs.select.result != 'success' || ", "", 1)
        self.assertNotEqual(job_sections(combined), job_sections(changed))

    def test_manual_wrapper_cannot_restart_automatic_fanout(self):
        text = (WORKFLOWS / "tor-reader-visual.yml").read_text()
        changed = text.replace("  workflow_dispatch:\n",
                               "  pull_request:\n    paths: ['crates/reading/**']\n")
        self.assertTrue(problems(changed, "reader_browser"))

    def test_rules_are_unconditional_and_deployment_is_separate(self):
        rules = (WORKFLOWS / "rules.yml").read_text()
        self.assertNotIn("needs: select", rules)
        self.assertIn("python3 -m unittest discover -s tests/architecture -v", rules)
        for filename in MANUAL_WORKFLOWS:
            with self.subTest(workflow=filename):
                deploy = (WORKFLOWS / filename).read_text()
                self.assertNotIn("ci-selection", deploy)
                self.assertNotIn("  push:\n", deploy)
                self.assertNotIn("  pull_request:\n", deploy)
                self.assertNotIn("  workflow_run:\n", deploy)
                self.assertIn("  workflow_dispatch:\n", deploy)

    def test_isolated_performance_workflow_does_not_contact_live_hosts(self):
        for filename in PERFORMANCE_WORKFLOWS:
            with self.subTest(workflow=filename):
                contents = (WORKFLOWS / filename).read_text()
                self.assertIn("  workflow_dispatch:\n", contents)
                self.assertIn("  pull_request:\n", contents)
                self.assertIn("    paths:", contents)
                self.assertNotIn("  push:\n", contents)
                self.assertNotIn("  pull_request_target:\n", contents)
                self.assertNotIn("secrets.", contents)
                self.assertIn("127.0.0.1:3089", contents)
                self.assertIn("postgres:18", contents)
                self.assertNotIn("tor-one-click-release", contents)
        harness = (ROOT / "tests/load/tor_http.js").read_text()
        self.assertIn("http://127.0.0.1:3089", harness)
        self.assertNotIn("__ENV.BASE_URL", harness)

    def test_reusable_planner_uses_complete_history_and_no_secrets(self):
        text = (WORKFLOWS / "ci-selection.yml").read_text()
        self.assertIn("fetch-depth: 0", text)
        self.assertIn("persist-credentials: false", text)
        self.assertIn("contents: read", text)
        self.assertNotIn("secrets.", text)
        self.assertNotIn("pull_request_target", text)
        self.assertIn("python3 tools/select_feature_tests.py", text)

    def test_no_unknown_workflows_or_missing_harness(self):
        known = ({filename for filename, _ in SUITES.values()}
                 | {CI_WORKFLOW, "rules.yml", "ci-selection.yml"}
                 | MANUAL_WORKFLOWS | PERFORMANCE_WORKFLOWS)
        self.assertEqual({p.name for p in WORKFLOWS.glob("*.yml")}, known)
        self.assertEqual(set(SUITES), set(SUITE_JOB_IDS))
        self.assertEqual(len(set(SUITE_JOB_IDS.values())), len(SUITES))
        for path, suites in TEST_SUITES.items():
            with self.subTest(path=path):
                self.assertTrue((ROOT / path).is_file())
                self.assertTrue(suites <= SUITES.keys())


if __name__ == "__main__":
    unittest.main()
