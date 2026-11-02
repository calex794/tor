"""Reject silent omissions in the permanent TOR required-check job."""
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import SUITE_JOB_IDS
from verify_ci_gate import GateError, verify


def evidence(selected=None, selector_result="success"):
    if selected is None:
        selected = set(SUITE_JOB_IDS)
    flags = {suite: suite in selected for suite in SUITE_JOB_IDS}
    jobs = {
        job_id: {"result": "success" if flags[suite] else "skipped"}
        for suite, job_id in SUITE_JOB_IDS.items()
    }
    jobs["select"] = {
        "result": selector_result, "outputs": {"suites": json.dumps(flags)},
    }
    return jobs


class CIRequiredGateTests(unittest.TestCase):
    def test_main_and_manual_require_complete_pack(self):
        for event in ("push", "workflow_dispatch"):
            with self.subTest(event=event):
                self.assertEqual(verify(event, evidence())["verified"], 14)
                with self.assertRaises(GateError):
                    verify(event, evidence({"rust"}))

    def test_pr_selection_can_skip_only_unselected_suites(self):
        partial = evidence({"rust", "postgres"})
        self.assertEqual(verify("pull_request", partial)["selected"], 2)
        self.assertEqual(verify("pull_request", evidence(set()))["verified"], 0)
        partial[SUITE_JOB_IDS["postgres"]]["result"] = "skipped"
        with self.assertRaises(GateError):
            verify("pull_request", partial)

    def test_failed_selector_runs_full_fallback_pack(self):
        full = evidence(selector_result="failure")
        self.assertTrue(verify("pull_request", full)["fallback_full"])
        full[SUITE_JOB_IDS["postgres"]]["result"] = "skipped"
        with self.assertRaises(GateError):
            verify("pull_request", full)
        with self.assertRaises(GateError):
            verify("push", evidence(selector_result="failure"))

    def test_invalid_dependency_or_unselected_failure_blocks(self):
        for action in ("missing", "extra", "nonobject", "failed_unselected"):
            with self.subTest(action=action):
                data = evidence(set())
                if action == "missing":
                    data.pop(SUITE_JOB_IDS["rust"])
                elif action == "extra":
                    data["other"] = {"result": "success"}
                elif action == "nonobject":
                    data[SUITE_JOB_IDS["rust"]] = None
                else:
                    data[SUITE_JOB_IDS["rust"]]["result"] = "failure"
                with self.assertRaises(GateError):
                    verify("pull_request", data)

    def test_bad_selector_json_or_types_blocks(self):
        invalid = ('{"rust":true}', '{"rust":"false"}', "bad json", "",
                   json.dumps({suite: 1 for suite in SUITE_JOB_IDS}))
        for output in invalid:
            with self.subTest(output=output):
                data = evidence({"rust"})
                data["select"]["outputs"]["suites"] = output
                with self.assertRaises(GateError):
                    verify("pull_request", data)

    def test_cancelled_selector_and_unknown_event_block(self):
        with self.assertRaises(GateError):
            verify("pull_request", evidence(selector_result="cancelled"))
        with self.assertRaises(GateError):
            verify("merge_group", evidence())


if __name__ == "__main__":
    unittest.main()
