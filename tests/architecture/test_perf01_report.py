"""PERF-01: evidence integrity for local k6 artifacts. No network or database."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import perf01_report as report

SHA = "0123456789abcdef" * 2 + "01234567"


def sample():
    def metric(values):
        return {"values": values}
    return {"metrics": {
        "http_reqs": metric({"count": 200, "rate": 23.5}),
        # Real k6 Rate metrics use value, not rate; successful requests are "fails".
        "http_req_failed": {"value": 0.02, "passes": 4, "fails": 196},
        "dropped_iterations": metric({"count": 3}),
        "http_req_duration": metric({"med": 12.5, "p(95)": 39, "p(99)": 50}),
        "tor_duration_catalog": metric({"med": 20, "p(95)": 40, "p(99)": 70}),
    }}


class PerfEvidence(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "diagnostic").mkdir()
        (self.root / "results.tsv").write_text("25\tPASS\n", encoding="utf-8")
        (self.root / "summary-25.json").write_text(json.dumps(sample()), encoding="utf-8")

    def test_complete_base_route_quantiles(self):
        result = report.build(self.root, SHA, "112233")
        self.assertEqual(result["report_state"], "MEASURED")
        stage = result["stages"][0]
        self.assertEqual(stage["evidence"], "PASS")
        self.assertEqual(stage["p50_ms"], 12.5)
        self.assertEqual(stage["p95_ms"], 39)
        self.assertEqual(stage["p99_ms"], 50)
        self.assertEqual(stage["route_latency"]["catalog"]["p95_ms"], 40)
        self.assertEqual(stage["actual_rps"], 23.5)
        self.assertIsNone(stage["sqlx_queries_per_request"])
        self.assertIsNone(result["unmeasured"]["postgresql_server_cpu_ms"])

    def test_real_k6_rate_value_zero_is_measured_not_missing(self):
        data = sample()
        data["metrics"]["http_req_failed"] = {
            "passes": 0, "fails": 200, "value": 0,
            "thresholds": {"rate<0.01": False},
        }
        (self.root / "summary-25.json").write_text(json.dumps(data), encoding="utf-8")
        stage = report.build(self.root, SHA)["stages"][0]
        self.assertEqual(stage["evidence"], "PASS")
        self.assertEqual(stage["http_error_rate"], 0.0)
        self.assertEqual(stage["dropped_iterations"], 3)

    def test_failed_high_rate_survives_with_complete_summary(self):
        (self.root / "diagnostic/results.tsv").write_text(
            "profile\ttarget_rps\toutcome\ncatalog\t500\tFAIL\n", encoding="utf-8")
        (self.root / "diagnostic/summary-catalog-500.json").write_text(
            json.dumps(sample()), encoding="utf-8")
        result = report.build(self.root, SHA)
        self.assertEqual(result["report_state"], "MEASURED")
        self.assertEqual(result["stages"][1]["evidence"], "FAIL")
        self.assertIn("FAIL", report.markdown(result))

    def test_missing_summary_cannot_claim_pass(self):
        (self.root / "summary-25.json").unlink()
        result = report.build(self.root, SHA)
        self.assertEqual(result["report_state"], "INCOMPLETE")
        self.assertEqual(result["stages"][0]["evidence"], "INCOMPLETE")
        self.assertIsNone(result["stages"][0]["p95_ms"])
        self.assertIsNone(result["stages"][0]["actual_rps"])

    def test_malformed_summary_fails_closed(self):
        (self.root / "summary-25.json").write_text("{oops", encoding="utf-8")
        self.assertEqual(report.build(self.root, SHA)["report_state"], "INCOMPLETE")

    def test_missing_metrics_are_not_zero(self):
        (self.root / "summary-25.json").write_text(
            json.dumps({"metrics": {"http_reqs": {"values": {"count": 1}}}}),
            encoding="utf-8")
        result = report.build(self.root, SHA)
        self.assertEqual(result["report_state"], "INCOMPLETE")
        self.assertIsNone(result["stages"][0]["http_error_rate"])
        self.assertIsNone(result["stages"][0]["dropped_iterations"])

    def test_invalid_number_rejected(self):
        data = sample()
        data["metrics"]["http_req_duration"]["values"]["p(95)"] = float("nan")
        data["metrics"]["http_req_failed"]["values"]["rate"] = -0.5
        (self.root / "summary-25.json").write_text(json.dumps(data), encoding="utf-8")
        stage = report.build(self.root, SHA)["stages"][0]
        self.assertEqual(stage["evidence"], "INCOMPLETE")
        self.assertIsNone(stage["p95_ms"])
        self.assertIsNone(stage["http_error_rate"])

    def test_duplicate_stage_refused(self):
        (self.root / "results.tsv").write_text("25\tPASS\n25\tPASS\n", encoding="utf-8")
        with self.assertRaises(report.EvidenceError):
            report.build(self.root, SHA)

    def test_invalid_profile_and_rate_refused(self):
        (self.root / "diagnostic/results.tsv").write_text(
            "profile\ttarget_rps\toutcome\n../../secret\t501\tPASS\n",
            encoding="utf-8")
        with self.assertRaises(report.EvidenceError):
            report.build(self.root, SHA)

    def test_empty_baseline_not_accidental_success(self):
        (self.root / "results.tsv").unlink()
        result = report.build(self.root, SHA)
        self.assertEqual(result["report_state"], "INCOMPLETE")
        self.assertEqual(result["stages"], [])

    def test_metadata_provenance(self):
        (self.root / "cpu_count.txt").write_text("2\n", encoding="ascii")
        r = report.build(self.root, SHA, "777")
        self.assertEqual(r["environment"]["runner_logical_cpus"], 2)
        self.assertEqual(r["source_sha"], SHA)
        self.assertEqual(r["github_run_id"], "777")
        self.assertNotIn("production", r["environment"]["target"])

    def test_invalid_sha_and_run_id(self):
        with self.assertRaises(report.EvidenceError):
            report.build(self.root, "short")
        with self.assertRaises(report.EvidenceError):
            report.build(self.root, SHA, "abc")


if __name__ == "__main__":
    unittest.main()
