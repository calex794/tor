"""ADAPT-00/02a: offline inventory and model; no runtime or external dependencies."""
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import adapt_limit_inventory as inventory
import adapt_pacing_model as model


class LimitInventory(unittest.TestCase):
    def test_fixture_covers_rules_and_does_not_emit_source_values(self):
        source = (
            "const MAX_BATCH: usize = 16;\n"
            'let secret = "DO_NOT_EMIT"; let key = env::var("TOR_POOL_MAX");\n'
            "Duration::from_millis(250); Semaphore::new(1);\n"
            "if !(1..=64).contains(&n) || x.len() > 64 { return; }\n"
            'sqlx::query("SELECT id FROM books LIMIT $2");\n'
            "permit_auth_attempt(&key, 8, 600);\n"
            "MemoryMax=256M\n"
        )
        results = inventory.scan_text("crates/web/src/example.rs", source)
        self.assertEqual({row["rule"] for row in results}, {kind for kind, _ in inventory.RULES})
        self.assertNotIn("DO_NOT_EMIT", json.dumps(results))
        self.assertTrue(all(row["classification"] == "unreviewed" for row in results))
        self.assertTrue(all(row["line"] > 0 and row["column"] > 0 for row in results))
        self.assertEqual(inventory.scan_text("x.rs", "fn simple() {}"), [])

    def test_scopes(self):
        self.assertEqual(inventory.scope("crates/books/tests/scale.rs"), "test")
        self.assertEqual(inventory.scope("ops/tor-web.service.example"), "operator_or_ci")
        self.assertEqual(inventory.scope("tools/adapt_limit_inventory.py"), "tool_or_reference")
        self.assertEqual(inventory.scope("crates/web/src/main.rs"), "source")

    def test_tracked_checkout_inventory(self):
        result = inventory.inventory(ROOT)
        self.assertEqual(result["tracked_files"], result["scanned_files"] + sum(result["skipped"].values()))
        self.assertEqual(result["candidate_count"], sum(result["by_rule"].values()))
        self.assertEqual(result["candidate_count"], sum(result["by_scope"].values()))
        self.assertGreater(result["scanned_by_extension"][".rs"], 0)
        self.assertGreater(result["scanned_by_extension"][".sql"], 0)
        self.assertTrue(any(c["path"] == "crates/web/src/recommendation_worker.rs"
                            and c["rule"] == "duration" for c in result["candidates"]))
        print("ADAPT_INVENTORY " + json.dumps(inventory.summary(result), sort_keys=True), flush=True)


class PacingModel(unittest.TestCase):
    def test_overload_bounds_and_slow_recovery(self):
        pacer = model.Pacer()
        self.assertEqual(pacer.after_progress("high"), 10)
        for _ in range(20):
            pacer.after_progress("high")
        self.assertEqual(pacer.delay, model.MAX_DELAY)
        self.assertEqual(pacer.after_progress("low"), model.MAX_DELAY)
        self.assertEqual(pacer.after_progress("low"), model.MAX_DELAY - model.MIN_DELAY)
        for _ in range(100):
            pacer.after_progress("low")
        self.assertEqual(pacer.delay, model.MIN_DELAY)

    def test_missing_pressure_and_hysteresis(self):
        pacer = model.Pacer()
        self.assertEqual(pacer.after_progress("missing"), model.MAX_DELAY)
        pacer.after_progress("low")
        pacer.after_progress("neutral")
        self.assertEqual(pacer.after_progress("low"), model.MAX_DELAY)
        with self.assertRaises(ValueError):
            pacer.after_progress("invalid")

    def test_comparable_deterministic_workloads_and_conservation(self):
        report = model.experiment()
        self.assertEqual(report, model.experiment())
        self.assertEqual(len(report["results"]), 10)
        for baseline, candidate in zip(report["results"][::2], report["results"][1::2]):
            for key in ("scenario", "horizon_ms", "reader_arrivals", "builder_arrivals"):
                self.assertEqual(baseline[key], candidate[key])
            for row in (baseline, candidate):
                self.assertEqual(row["reader_arrivals"], row["reader_completed"] + row["reader_unfinished"])
                self.assertEqual(row["builder_arrivals"], row["builder_completed"] + row["builder_unfinished"])
                if row["max_progress_delay_ms"] is not None:
                    self.assertLessEqual(row["max_progress_delay_ms"], 5000)
                    self.assertGreaterEqual(row["min_progress_delay_ms"], 250)
        print("ADAPT_MODEL " + json.dumps(report, sort_keys=True), flush=True)

    def test_empty_metrics_are_not_zero_and_idle_has_no_work(self):
        idle = model.simulate("idle", True)
        self.assertIsNone(idle["reader_p95_model_ms"])
        self.assertIsNone(idle["pool_wait_p95_model_ms"])
        self.assertEqual(idle["builder_completed"], 0)
        self.assertIsNone(idle["max_progress_delay_ms"])


if __name__ == "__main__":
    unittest.main()
