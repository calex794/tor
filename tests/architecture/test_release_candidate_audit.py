"""I-12: reject stale, partial, skipped or cross-SHA suite evidence."""
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import (
    CI_REQUIRED_JOB, CI_WORKFLOW, SUITE_JOB_IDS, SUITE_JOB_MATRICES,
    suite_api_job_names,
)
from audit_release_candidate import (
    EvidenceError, collect, evaluate_ci, latest_push_runs, workflow_names,
)

SHA = "a" * 40
OTHER = "b" * 40

# Independent fixture: the actual Jobs API shape of successful main run
# 45645215690, NOT generated from the production registry under test.
INTEGRATION_JOBS = (
    "preflight", "select / select", "visual", "pg-reading-analytics",
    "pg-rec-browser (current)", "migrations", "postgres-runtime (reading)",
    "postgres-runtime (foundation)", "crypto", "demo (tests)", "reader-browser",
    "sealed-remote-copy", "pg-rec-browser (historical)", "paid-private-e2e",
    "purchased-v1-recovery", "author-cover", "demo (release)",
    "postgres-runtime (commerce)", "image", "s3-cover-build", "s3-provider",
    "f02-100k", "ci-required",
)


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
        names = (["select / select", "preflight", *(
            name for name in INTEGRATION_JOBS
            if name not in {"select / select", "preflight", CI_REQUIRED_JOB}
        ), CI_REQUIRED_JOB] if filename == CI_WORKFLOW else ["rules"])
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
        self.assertEqual(len(jobs[2]), len(INTEGRATION_JOBS))

    def test_every_real_suite_and_rules_is_required(self):
        expected, runs, jobs = fixtures()
        receipt = evaluate_ci(runs, jobs, SHA, expected)
        self.assertTrue(receipt["ci_complete"])
        self.assertEqual(receipt["verified_workflows"], 2)
        self.assertEqual(receipt["verified_suites"], len(SUITE_JOB_IDS))
        self.assertEqual(receipt["required_suites"], len(SUITE_JOB_IDS))
        self.assertFalse(receipt["release_approved"])
        self.assertEqual(receipt["scope"], "CI-only, not a production or product-parity approval")

    def test_release_registry_matches_actual_workflow_matrix(self):
        workflow = (ROOT / ".github" / "workflows" / CI_WORKFLOW).read_text()
        body = workflow.split("\njobs:\n", 1)[1]
        entries = re.split(r"(?m)^  ([A-Za-z0-9_-]+):\n", body)
        sections = dict(zip(entries[1::2], entries[2::2]))
        self.assertEqual(set(sections),
                         {"select", "preflight", CI_REQUIRED_JOB} | set(SUITE_JOB_IDS.values()))
        for suite, job_id in SUITE_JOB_IDS.items():
            with self.subTest(suite=suite):
                section = sections[job_id]
                self.assertNotRegex(section, r"(?m)^    name:")
                strategy = re.search(r"(?m)^    strategy:\n(?:^ {6,}[^\n]*\n)+", section)
                if suite in SUITE_JOB_MATRICES:
                    axis, values = SUITE_JOB_MATRICES[suite]
                    self.assertIsNotNone(strategy)
                    self.assertEqual(strategy.group(0),
                                     "    strategy:\n      fail-fast: false\n"
                                     f"      matrix:\n        {axis}: [{', '.join(values)}]\n")
                else:
                    self.assertIsNone(strategy)
        api_names = {name for names in suite_api_job_names().values() for name in names}
        self.assertEqual(api_names | {"select / select", "preflight", CI_REQUIRED_JOB},
                         set(INTEGRATION_JOBS))

    def test_each_matrix_child_is_independently_mandatory(self):
        children = {
            "rust": ("demo (tests)", "demo (release)"),
            "postgres": ("postgres-runtime (foundation)", "postgres-runtime (reading)",
                         "postgres-runtime (commerce)"),
            "rec_browser": ("pg-rec-browser (current)", "pg-rec-browser (historical)"),
        }
        for suite, names in children.items():
            for name in names:
                for action in ("missing", "failed", "skipped", "pending", "old-name"):
                    with self.subTest(suite=suite, name=name, action=action):
                        expected, runs, jobs = fixtures()
                        job = next(row for row in jobs[2] if row["name"] == name)
                        if action == "missing":
                            jobs[2].remove(job)
                        elif action == "old-name":
                            job["name"] = SUITE_JOB_IDS[suite]
                        elif action == "pending":
                            job.update(status="in_progress", conclusion=None)
                        else:
                            job["conclusion"] = {"failed": "failure", "skipped": "skipped"}[action]
                        report = evaluate_ci(runs, jobs, SHA, expected)
                        self.assertFalse(report["ci_complete"])
                        self.assertEqual(report["verified_suites"], len(SUITE_JOB_IDS) - 1)
                        self.assertNotEqual(report["suite_checks"][suite]["state"], "verified")

    def test_matrix_suite_receipt_preserves_all_children(self):
        expected, runs, jobs = fixtures()
        report = collect(MockAPI(runs, jobs), SHA, ROOT)
        self.assertTrue(report["ci_complete"])
        self.assertEqual(report["suite_checks"]["postgres"]["jobs"], {
            "postgres-runtime (foundation)": "verified",
            "postgres-runtime (reading)": "verified",
            "postgres-runtime (commerce)": "verified",
        })
        self.assertEqual(report["verified_suites"], 16)
        self.assertEqual(len(jobs[2]), 23)

    def test_missing_workflow_or_stale_sha_fails(self):
        expected, runs, jobs = fixtures()
        self.assertFalse(evaluate_ci(runs[:-1], jobs, SHA, expected)["ci_complete"])
        runs[-1]["head_sha"] = OTHER
        self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_missing_one_suite_cannot_pass_a_green_wrapper(self):
        expected, runs, jobs = fixtures()
        jobs[2].pop(-2)
        report = evaluate_ci(runs, jobs, SHA, expected)
        self.assertFalse(report["ci_complete"])
        self.assertIn("missing/unknown jobs", " ".join(report["blockers"]))

    def test_missing_or_unsuccessful_required_gate_blocks_main(self):
        for action in ("missing", "failed", "skipped", "pending"):
            with self.subTest(action=action):
                expected, runs, jobs = fixtures()
                if action == "missing":
                    jobs[2] = [job for job in jobs[2]
                               if job["name"] != CI_REQUIRED_JOB]
                else:
                    gate = jobs[2][-1]
                    if action == "failed":
                        gate["conclusion"] = "failure"
                    elif action == "skipped":
                        gate["conclusion"] = "skipped"
                    else:
                        gate["status"] = "in_progress"
                        gate["conclusion"] = None
                self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

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
                jobs[2][-2] = {"name": jobs[2][-2]["name"],
                               "status": status, "conclusion": conclusion}
                self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_unknown_or_duplicate_suite_job_blocks(self):
        for action in ("unknown", "duplicate"):
            with self.subTest(action=action):
                expected, runs, jobs = fixtures()
                job = (dict(jobs[2][-2]) if action == "duplicate" else
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

    def test_old_sixteen_job_receipt_without_preflight_is_rejected(self):
        expected, runs, jobs = fixtures()
        jobs[2] = [job for job in jobs[2] if job["name"] != "preflight"]
        report = evaluate_ci(runs, jobs, SHA, expected)
        self.assertFalse(report["ci_complete"])
        self.assertIn("preflight", " ".join(report["blockers"]))
        self.assertFalse(report["release_approved"])

    def test_bad_preflight_cannot_be_hidden_by_green_wrapper(self):
        states = (("completed", "failure"), ("completed", "skipped"),
                  ("completed", "cancelled"), ("completed", "neutral"),
                  ("completed", "timed_out"), ("in_progress", None), ("queued", None))
        for status, conclusion in states:
            with self.subTest(status=status, conclusion=conclusion):
                expected, runs, jobs = fixtures()
                preflight = next(job for job in jobs[2] if job["name"] == "preflight")
                preflight.update(status=status, conclusion=conclusion)
                report = evaluate_ci(runs, jobs, SHA, expected)
                self.assertFalse(report["ci_complete"])
                self.assertFalse(report["release_approved"])

    def test_duplicate_or_renamed_preflight_fails(self):
        for action in ("duplicate", "rename"):
            with self.subTest(action=action):
                expected, runs, jobs = fixtures()
                preflight = next(job for job in jobs[2] if job["name"] == "preflight")
                if action == "duplicate":
                    jobs[2].append(dict(preflight))
                else:
                    preflight["name"] = "preflight-untrusted"
                self.assertFalse(evaluate_ci(runs, jobs, SHA, expected)["ci_complete"])

    def test_preflight_is_not_counted_as_an_additional_suite(self):
        expected, runs, jobs = fixtures()
        jobs[2].reverse()
        report = collect(MockAPI(runs, jobs), SHA, ROOT)
        integration_title = next(title for title, path in expected.items() if path == CI_WORKFLOW)
        self.assertTrue(report["ci_complete"])
        self.assertEqual(report["checks"][integration_title]["jobs"], len(INTEGRATION_JOBS))
        self.assertEqual(report["required_suites"], len(SUITE_JOB_IDS))
        self.assertFalse(report["release_approved"])


if __name__ == "__main__":
    unittest.main()
