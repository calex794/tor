"""PERF-02g: parser must not invent measured pool wait from missing samples."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import perf02_pool_evidence as evidence  # noqa: E402


class Perf02PoolEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        stages = []
        results = ["target_rps\toutcome"]
        samples = []
        for idx, rate in enumerate(evidence.ALLOWED_RATES):
            minute = idx * 2
            start = f"2026-11-15T08:{minute:02d}:00Z"
            finish = f"2026-11-15T08:{minute:02d}:15Z"
            stages.extend([f"{start}\t{rate}\tstart", f"{finish}\t{rate}\tfinish"])
            results.append(f"{rate}\tPASS")
            epoch = evidence.ms_timestamp(start)
            for n in range(5):
                samples.append(
                    f"TOR_PERF02_POOL_SAMPLE epoch_ms={epoch + 1000 + n * 10} "
                    "status=200 http_us=4000 queries=13 "
                    f"acquires=14 acquire_total_us={100 + n} acquire_max_us=60"
                )
            metric = {"values": {"med": 2.0, "p(95)": 3.0, "p(99)": 4.0}}
            (self.root / f"summary-{rate}.json").write_text(
                json.dumps({"metrics": {
                    "perf02_book_guest_ms": metric,
                    "perf02_book_reader_ms": metric,
                }}), encoding="utf-8"
            )
        (self.root / "stages.tsv").write_text("\n".join(stages) + "\n", encoding="utf-8")
        (self.root / "results.tsv").write_text("\n".join(results) + "\n", encoding="utf-8")
        (self.root / "server.log").write_text("\n".join(samples) + "\n", encoding="utf-8")

    def test_per_stage_successful_acquisitions_are_numeric(self):
        result = evidence.summarize(self.root)
        self.assertEqual(len(result["stages"]), 4)
        for item in result["stages"]:
            self.assertEqual(item["sampled_book_requests"], 5)
            self.assertEqual(item["with_successful_acquire"], 5)
            self.assertEqual(item["acquire_total_us_p95"], 104)
            self.assertEqual(item["acquire_max_us_p95"], 60)
            self.assertEqual(item["acquires_per_request_p95"], 14)
            self.assertEqual(item["k6_latency"]["reader"]["p95_ms"], 3.0)
        self.assertNotIn("token", json.dumps(result))
        self.assertNotIn("trace_id", json.dumps(result))

    def test_empty_successful_acquisitions_are_not_zero_wait(self):
        text = (self.root / "server.log").read_text(encoding="utf-8")
        (self.root / "server.log").write_text(
            text.replace("acquires=14", "acquires=0")
            .replace("acquire_total_us=100", "acquire_total_us=0")
            .replace("acquire_total_us=101", "acquire_total_us=0")
            .replace("acquire_total_us=102", "acquire_total_us=0")
            .replace("acquire_total_us=103", "acquire_total_us=0")
            .replace("acquire_total_us=104", "acquire_total_us=0")
            .replace("acquire_max_us=60", "acquire_max_us=0"),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "no trustworthy"):
            evidence.summarize(self.root)

    def test_rejects_non_numeric_sample_fields(self):
        with (self.root / "server.log").open("a", encoding="utf-8") as file:
            file.write("TOR_PERF02_POOL_SAMPLE session=private-secret\n")
        with self.assertRaisesRegex(evidence.EvidenceError, "malformed numeric"):
            evidence.summarize(self.root)

    def test_rejects_duplicate_stage(self):
        with (self.root / "stages.tsv").open("a", encoding="utf-8") as file:
            file.write("2026-11-15T08:00:00Z\t20\tstart\n")
        with self.assertRaisesRegex(evidence.EvidenceError, "duplicate stage"):
            evidence.summarize(self.root)


if __name__ == "__main__":
    unittest.main()
