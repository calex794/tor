"""I-12: reject stale, partial, skipped or cross-SHA suite evidence."""
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import CI_WORKFLOW, SUITE_JOB_IDS
from audit_release_candidate import (
    EvidenceError, collect, evaluate_ci, latest_push_runs, workflow_names,
)

SHA = "a" * 40
OTHER = "b" * 40


def fixtures():
    expected = workflow_names(ROOT)
    runs = []
    jobs = {}
    for number, (title, filename) in enumerate(expected.items(), start=1):
        runs.append({
            "id": number, "name": title, "event": "push", "head_sha": SHA,
            "status": "completed", "conclusion": "success",
            "created_at": "2026-10-29T00:00:00Z", "run_attempt": 1,
        })
        names = (["select / select", *SUITE_JOB_IDS.values()]
                 if filename == CI_WORKFLOW else ["rules"])
        jobs[number] = [
            {"name": name, "status": "completed", "conclusion": "success"}
            for name in names
        ]
    return expected, runs, jobs


class MockAPI:
    def __init__(self, runs, jobs, main=SHA, total=None, main_after=None,
                 job_total_override=None):
        self.runs, self.jobs, self.main = runs, jobs, main
        self.main_after = main_after
        self.main_reads = 0
        self.total = len(runs) if total is None else total
        self.job_total_override = job_total_override

    def get(self, path, params=None):
        if path.endswith("/git/ref/heads/main"):
            self.main_reads += 1
            sha = self.main_after if self.main_reads > 1 and self.main_after else self.main
            return {"ref": "refs/heads/main", "object": {"sha": sha}}
        if path.endswith("/actions/runs"):
            return {"workflow_runs": self.runs, "total_count": self.total}
        if path.endswith("/jobs"):
            run_id = int(path.rsplit("/", 2)[-2])
            rows = self.jobs.get(run_id, [])
            count = self.job_total_override if self.job_total_override is not None else len(rows)
            return {"jobs": rows, "total_count": count}
        raise AssertionError(path)


class ReleaseCandidateAuditTests(unittest.TestCase):
    def test_registry_requires_rules_and_one_integration_workflow(self):
        expected, _, jobs = fixtures()
        self.assertEqual(set(expected.values()), {"rules.yml", CI_WORKFLOW})
        self.assertEqual(len(expected), 2)
        self.assertEqual(len(jobs[2]), len(SUITE_JOB_IDS) + 1)

    def test_every_real_suite_and_rules_is_required(self):
        expected, runs, jobs = fixtures()
        receipt = evaluate_ci(runs, jobs, SHA, expected)
        self.assertTrue(receipt["ci_complete"])
        self.assertEqual(receipt["verified_workflows"], 2)
        self.assertEqual(receipt["verified_suites"], 14)
        self.assertEqual(receipt["required_suites"], 14)
        self.assertFalse(receipt["release_approved"])
        self.assertEqual(receipt["scope"], "CI-only, not a production or product-parity approval")

    def test_missing_workflow_or_stale_sha_fails(self):
        expected, runs, jobs = fixtures()
        self.assertFalse(evaluate_ci(runs[:-1], jobs, SHA, expected)["ci_complete"])
        runs[-1]["head_sha"] = OTHER
        self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_missing_one_suite_cannot_pass_a_green_wrapper(self):
        expected, runs, jobs = fixtures()
        jobs[2].pop()
        report = evaluate_ci(runs, jobs, SHA, expected)
        self.assertFalse(report["ci_complete"])
        self.assertIn("missing/unknown jobs", " ".join(report["blockers"]))

    def test_selector_only_success_cannot_hide_skipped_tests(self):
        expected, runs, jobs = fixtures()
        jobs[2] = jobs[2][:1]
        report = evaluate_ci(runs, jobs, SHA, expected)
        self.assertFalse(report["ci_complete"])
        self.assertIn("selector-only success", " ".join(report["blockers"]))

    def test_missing_failed_or_duplicate_selector_blocks(self):
        for selector_action in ("remove", "duplicate", "fail", "rename"):
            with self.subTest(selector=selector_action):
                expected, runs, jobs = fixtures()
                if selector_action == "remove":
                    jobs[2].pop(0)
                elif selector_action == "duplicate":
                    jobs[2].insert(1, dict(jobs[2][0]))
                elif selector_action == "fail":
                    jobs[2][0]["conclusion"] = "failure"
                else:
                    jobs[2][0]["name"] = "select / attacker"
                self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_skipped_pending_or_failed_suite_blocks(self):
        for status, conclusion in (("completed", "skipped"), ("in_progress", None),
                                   ("completed", "failure")):
            with self.subTest(status=status, conclusion=conclusion):
                expected, runs, jobs = fixtures()
                jobs[2][-1] = {"name": jobs[2][-1]["name"],
                               "status": status, "conclusion": conclusion}
                self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_unknown_or_duplicate_suite_job_blocks(self):
        for action in ("unknown", "duplicate"):
            with self.subTest(action=action):
                expected, runs, jobs = fixtures()
                job = (dict(jobs[2][-1]) if action == "duplicate" else
                       {"name": "unexpected-test", "status": "completed", "conclusion": "success"})
                jobs[2].append(job)
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
        runs.append(dict(runs[0], id=300, name="unregistered push workflow"))
        self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])
        runs.pop()
        jobs.pop(2)
        self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_invalid_sha_or_incomplete_registry_rejected(self):
        expected, runs, jobs = fixtures()
        with self.assertRaises(EvidenceError):
            evaluate_ci(runs, jobs, "not-a-SHA", expected)
        with self.assertRaises(EvidenceError):
            evaluate_ci(runs, jobs, SHA, {})
        with self.assertRaises(EvidenceError):
            evaluate_ci(runs, jobs, SHA, {"Repository rules": "rules.yml"})

    def test_main_moved_fails_closed_without_fetching_jobs(self):
        _, runs, jobs = fixtures()
        with self.assertRaises(EvidenceError):
            collect(MockAPI(runs, jobs, main=OTHER), SHA, ROOT)

    def test_main_moves_during_job_collection_fails_closed(self):
        _, runs, jobs = fixtures()
        api = MockAPI(runs, jobs, main=SHA, main_after=OTHER)
        with self.assertRaises(EvidenceError):
            collect(api, SHA, ROOT)
        self.assertEqual(api.main_reads, 2)

    def test_incomplete_workflow_or_job_pagination_fails_closed(self):
        _, runs, jobs = fixtures()
        with self.assertRaises(EvidenceError):
            collect(MockAPI(runs, jobs, total=100), SHA, ROOT)
        with self.assertRaises(EvidenceError):
            collect(MockAPI(runs, jobs, job_total_override=100), SHA, ROOT)

    def test_exact_main_with_all_job_evidence_is_ci_ready(self):
        _, runs, jobs = fixtures()
        report = collect(MockAPI(runs, jobs), SHA, ROOT)
        self.assertTrue(report["ci_complete"])
        self.assertFalse(report["release_approved"])


if __name__ == "__main__":
    unittest.main()
