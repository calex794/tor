"""I-09: offline tests for immutable SHA, pagination and CI job timing."""
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "tools"))
from ci_metrics import MetricsError, load_group, percentile, difference, comparison_blocker

SHA_A = "a" * 40
SHA_B = "b" * 40

class API:
    def __init__(self, pending=False, mismatch=False, failed=False):
        self.pending = pending
        self.mismatch = mismatch
        self.failed = failed
    def get(self, path, query=None):
        if path.endswith("/actions/runs"):
            return {"total_count": 1, "workflow_runs": [
                {"id": 3, "name": "rules", "event": "push",
                 "head_sha": SHA_B if self.mismatch else SHA_A,
                 "status": "in_progress" if self.pending else "completed",
                 "conclusion": None if self.pending else ("failure" if self.failed else "success")}]}
        if path.endswith("/jobs"):
            return {"total_count": 1, "jobs": [
                {"id": 8, "name": "rules", "run_id": 3,
                 "created_at": "2026-10-27T01:00:00Z",
                 "started_at": "2026-10-27T01:01:00Z",
                 "completed_at": None if self.pending else "2026-10-27T01:03:00Z",
                 "conclusion": None if self.pending else ("failure" if self.failed else "success")}]}
        if path.endswith("/3"):
            return {"id": 3, "head_sha": SHA_A, "status": "completed", "conclusion": "success"}
        raise AssertionError(path)

class MetricsTest(unittest.TestCase):
    def test_percentile(self):
        self.assertEqual(percentile([], .5), None)
        self.assertEqual(percentile([20, 10], .5), 15)
        self.assertAlmostEqual(percentile([20, 10], .95), 19.5)
    def test_queue_and_runtime(self):
        result = load_group(API(), "calex794/tor", "target", SHA_A)
        self.assertTrue(result["complete"])
        self.assertEqual(result["queue_p50_seconds"], 60)
        self.assertEqual(result["run_p95_seconds"], 120)
        self.assertEqual(result["runner_minutes"], 2)
    def test_incomplete_is_not_final(self):
        result = load_group(API(pending=True), "calex794/tor", "target", SHA_A)
        self.assertFalse(result["complete"])
        self.assertEqual(result["jobs_completed"], 0)
    def test_failed_run_cannot_prove_speedup(self):
        result = load_group(API(failed=True), "calex794/tor", "target", SHA_A)
        self.assertFalse(result["complete"])
        self.assertEqual(result["runs"][0]["conclusion"], "failure")
    def test_mismatch_fails_closed(self):
        with self.assertRaises(MetricsError):
            load_group(API(mismatch=True), "calex794/tor", "target", SHA_A)
    def test_repeated_id_is_rejected(self):
        with self.assertRaises(MetricsError):
            load_group(API(), "calex794/tor", "target", run_ids=[3, 3])
    def test_signed_comparison(self):
        before = load_group(API(), "calex794/tor", "baseline", SHA_A)
        after = dict(before, runner_minutes=1)
        self.assertEqual(difference(before, after)["runner_minutes"]["percent"], -50)

    def test_exact_same_executed_workflows_are_comparable(self):
        before = load_group(API(), "calex794/tor", "baseline", SHA_A)
        after = dict(before, runner_minutes=1)
        self.assertEqual(before["event"], "push")
        self.assertEqual(before["executed_jobs"], ["rules / rules"])
        self.assertEqual(before["workflow_names"], ["rules"])
        self.assertIsNone(comparison_blocker(before, after))

    def test_skipped_jobs_never_prove_speedup_even_with_successful_wrapper(self):
        before = load_group(API(), "calex794/tor", "baseline", SHA_A)
        after = dict(before, runner_minutes=0, skipped_jobs=["rules / real-tests"],
                     jobs_skipped=1)
        self.assertTrue(after["complete"])
        self.assertIn("skipped", comparison_blocker(before, after))
        self.assertIn("skipped", comparison_blocker(after, after))

    def test_mismatched_job_names_cannot_prove_speedup(self):
        before = load_group(API(), "calex794/tor", "baseline", SHA_A)
        after = dict(before, executed_jobs=["rules / fast-noop"], runner_minutes=0)
        self.assertIn("executed job", comparison_blocker(before, after))

    def test_mismatched_workflows_or_event_cannot_prove_speedup(self):
        before = load_group(API(), "calex794/tor", "baseline", SHA_A)
        renamed = dict(before, workflow_names=["not-the-actual-workflow"])
        self.assertIn("workflow", comparison_blocker(before, renamed))
        wrong_event = dict(before, event="pull_request")
        self.assertIn("events", comparison_blocker(before, wrong_event))

    def test_failed_or_incomplete_runs_cannot_prove_speedup(self):
        baseline = load_group(API(), "calex794/tor", "baseline", SHA_A)
        incomplete = load_group(API(failed=True), "calex794/tor", "target", SHA_A)
        self.assertIn("unfinished", comparison_blocker(baseline, incomplete))

    def test_successful_wrapper_with_pending_job_does_not_count_as_complete(self):
        class PendingJob(API):
            def get(self, path, query=None):
                response = super().get(path, query)
                if path.endswith("/jobs"):
                    response["jobs"][0]["conclusion"] = None
                return response
        result = load_group(PendingJob(), "calex794/tor", "target", SHA_A)
        self.assertFalse(result["complete"])

    def test_nameless_job_cannot_be_compared(self):
        class NamelessJob(API):
            def get(self, path, query=None):
                response = super().get(path, query)
                if path.endswith("/jobs"):
                    response["jobs"][0].pop("name", None)
                return response
        with self.assertRaises(MetricsError):
            load_group(NamelessJob(), "calex794/tor", "target", SHA_A)


if __name__ == "__main__":
    unittest.main()
