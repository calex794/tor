"""Architecture and evidence regressions for PERF-02h mixed business load."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import perf02h_mixed_evidence as evidence  # noqa: E402


class MixedEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.fixture("25\tPASS\n50\tPASS\n100\tPASS\n")
        for rate in evidence.RATES:
            self.write_summary(rate)

    def fixture(self, rows):
        (self.root / "results.tsv").write_text(
            "journeys_per_second\toutcome\n" + rows, encoding="utf-8"
        )

    def write_summary(self, rate, *, nested=False, negative=False):
        values = {
            "med": -1.0 if negative else 2.1,
            "p(95)": 4.5,
            "p(99)": 6.7,
        }
        metric = {"values": values} if nested else values
        (self.root / f"summary-{rate}.json").write_text(
            json.dumps({"metrics": {name: metric for name in evidence.MEASURES.values()}}),
            encoding="utf-8",
        )

    def test_real_k6_flat_and_nested_trends(self):
        self.write_summary(50, nested=True)
        output = evidence.summarize(self.root)
        self.assertEqual([x["target_journeys_per_second"] for x in output["stages"]],
                         [25, 50, 100])
        self.assertEqual(output["stages"][0]["http_latency"]["paid_purchase"]["p95_ms"], 4.5)
        self.assertEqual(output["stages"][1]["http_latency"]["free_progress_write"]["p99_ms"], 6.7)
        self.assertEqual(output["stages"][2]["paid_journeys_per_second"], 8)
        self.assertNotIn("password", json.dumps(output))
        self.assertNotIn("session", json.dumps(output))

    def test_optional_failed_stage_missing_metrics_is_null_not_zero(self):
        self.fixture("25\tPASS\n50\tPASS\n100\tFAIL\n")
        (self.root / "summary-100.json").unlink()
        output = evidence.summarize(self.root)
        self.assertIsNone(output["stages"][2]["http_latency"])
        self.assertEqual(output["stages"][2]["k6_outcome"], "FAIL")

    def test_mandatory_baseline_failure_is_rejected(self):
        self.fixture("25\tFAIL\n50\tPASS\n100\tPASS\n")
        with self.assertRaisesRegex(evidence.EvidenceError, "baseline"):
            evidence.summarize(self.root)

    def test_success_missing_k6_metric_is_rejected(self):
        self.write_summary(100, negative=True)
        with self.assertRaisesRegex(evidence.EvidenceError, "missing required"):
            evidence.summarize(self.root)

    def test_duplicate_rates_are_rejected(self):
        self.fixture("25\tPASS\n50\tPASS\n50\tPASS\n")
        with self.assertRaisesRegex(evidence.EvidenceError, "duplicate"):
            evidence.summarize(self.root)

    def test_missing_summary_for_success_is_not_zero(self):
        (self.root / "summary-50.json").unlink()
        with self.assertRaisesRegex(evidence.EvidenceError, "missing required"):
            evidence.summarize(self.root)


if __name__ == "__main__":
    unittest.main()
