"""I-12: deny stale, partial, selector-only or cross-SHA release evidence."""
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from audit_release_candidate import (
    EvidenceError, collect, evaluate_ci, latest_push_runs, workflow_names,
)

SHA = "a" * 40
OTHER = "b" * 40


def fixtures():
    expected = workflow_names(ROOT)
    runs = []
    jobs = {}
    for number, title in enumerate(expected, start=1):
        runs.append({
            "id": number, "name": title, "event": "push", "head_sha": SHA,
            "status": "completed", "conclusion": "success",
            "created_at": "2026-10-29T00:00:00Z", "run_attempt": 1,
        })
        base = [{"name": "verify", "status": "completed", "conclusion": "success"}]
        if title != "Repository rules":
            base.insert(0, {
                "name": "select / select", "status": "completed", "conclusion": "success",
            })
        jobs[number] = base
    return expected, runs, jobs


class MockAPI:
    def __init__(self, runs, jobs, main=SHA, total=None):
        self.runs, self.jobs, self.main = runs, jobs, main
        self.total = len(runs) if total is None else total

    def get(self, path, params=None):
        if path.endswith("/git/ref/heads/main"):
            return {"ref": "refs/heads/main", "object": {"sha": self.main}}
        if path.endswith("/actions/runs"):
            return {"workflow_runs": self.runs, "total_count": self.total}
        if path.endswith("/jobs"):
            run_id = int(path.rsplit("/", 2)[-2])
            rows = self.jobs.get(run_id, [])
            return {"jobs": rows, "total_count": len(rows)}
        raise AssertionError(path)


class ReleaseCandidateAuditTests(unittest.TestCase):
    def test_registered_workflows_include_rules_and_all_feature_suites(self):
        expected, _, _ = fixtures()
        self.assertEqual(len(expected), 15)
        self.assertIn("Repository rules", expected)
        self.assertNotIn("TOR — Обновить сайт", expected)

    def test_only_complete_job_verified_main_is_ci_ready(self):
        expected, runs, jobs = fixtures()
        receipt = evaluate_ci(runs, jobs, SHA, expected)
        self.assertTrue(receipt["ci_complete"])
        self.assertEqual(receipt["verified_workflows"], 15)
        self.assertFalse(receipt["release_approved"])
        self.assertEqual(receipt["scope"], "CI-only, not a production or product-parity approval")

    def test_missing_workflow_or_stale_sha_never_passes(self):
        expected, runs, jobs = fixtures()
        self.assertFalse(evaluate_ci(runs[:-1], jobs, SHA, expected)["ci_complete"])
        runs[-1]["head_sha"] = OTHER
        self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_selector_only_success_cannot_hide_skipped_real_tests(self):
        expected, runs, jobs = fixtures()
        jobs[2] = [jobs[2][0]]
        report = evaluate_ci(runs, jobs, SHA, expected)
        self.assertFalse(report["ci_complete"])
        self.assertIn("selector-only success", " ".join(report["blockers"]))

    def test_skipped_and_pending_jobs_block_even_when_wrapper_is_green(self):
        for state, conclusion in (("completed", "skipped"), ("in_progress", None),
                                  ("completed", "failure")):
            with self.subTest(state=state, conclusion=conclusion):
                expected, runs, jobs = fixtures()
                jobs[2][-1] = {"name": "verify", "status": state, "conclusion": conclusion}
                self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_older_failure_cannot_replace_latest_successful_rerun(self):
        expected, runs, jobs = fixtures()
        stale = dict(runs[0], id=999, created_at="2026-10-28T00:00:00Z",
                     conclusion="failure")
        runs.append(stale)
        self.assertEqual(latest_push_runs(runs, SHA)[runs[0]["name"]]["id"], runs[0]["id"])
        self.assertTrue(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])
        latest = dict(runs[0], id=1000, created_at="2026-10-30T00:00:00Z",
                      conclusion="failure")
        runs.append(latest)
        self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_unregistered_push_and_missing_job_evidence_block(self):
        expected, runs, jobs = fixtures()
        runs.append(dict(runs[0], id=300, name="new unreviewed push workflow"))
        self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])
        runs.pop()
        jobs.pop(3)
        self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_empty_registry_or_malformed_sha_is_rejected(self):
        expected, runs, jobs = fixtures()
        with self.assertRaises(EvidenceError):
            evaluate_ci(runs, jobs, "not-a-SHA", expected)
        with self.assertRaises(EvidenceError):
            evaluate_ci(runs, jobs, SHA, {})

    def test_main_moved_fails_closed_without_requesting_jobs(self):
        _, runs, jobs = fixtures()
        with self.assertRaises(EvidenceError):
            collect(MockAPI(runs, jobs, main=OTHER), SHA, ROOT)

    def test_incomplete_workflow_pagination_fails_closed(self):
        _, runs, jobs = fixtures()
        with self.assertRaises(EvidenceError):
            collect(MockAPI(runs, jobs, total=100), SHA, ROOT)

    def test_exact_main_with_all_job_evidence_is_ci_ready(self):
        _, runs, jobs = fixtures()
        result = collect(MockAPI(runs, jobs), SHA, ROOT)
        self.assertTrue(result["ci_complete"])
        self.assertFalse(result["release_approved"])


if __name__ == "__main__":
    unittest.main()
