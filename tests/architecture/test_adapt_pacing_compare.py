"""ADAPT-02b: comparative evidence and pressure safety, offline only."""
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import adapt_pacing_compare as compare
import adapt_pacing_model as model


class PacingComparison(unittest.TestCase):
    def test_high_pressure_never_accelerates_even_after_missing_signals(self):
        for mode in compare.MODES:
            candidate = compare.Candidate(mode)
            last = candidate.delay
            for _ in range(20):
                delay = candidate.after_progress("high", 1000, 1000)
                self.assertGreaterEqual(delay, last)
                self.assertLessEqual(delay, model.MAX_DELAY)
                last = delay
            self.assertEqual(candidate.after_progress("missing", 1000, 1000), model.MAX_DELAY)
            self.assertEqual(candidate.after_progress("high", 1000, 1000), model.MAX_DELAY)

    def test_recovery_is_bounded_and_age_only_accelerates_on_low(self):
        for mode in compare.MODES:
            candidate = compare.Candidate(mode)
            candidate.after_progress("missing", 0, 0)
            self.assertEqual(candidate.after_progress("neutral", 1000, 1000), model.MAX_DELAY)
            candidate.after_progress("low", 1000, 1000)
            expected = model.MAX_DELAY if mode == "fast_recovery" else model.MIN_DELAY
            self.assertEqual(candidate.delay, expected)
            for _ in range(20):
                candidate.after_progress("low", 0, 0)
            self.assertEqual(candidate.delay, model.MIN_DELAY)

    def test_existing_model_and_comparable_holdouts(self):
        report = compare.experiment()
        self.assertEqual(report, compare.experiment())
        self.assertEqual(len(report["development"]), 25)
        self.assertEqual(len(report["holdout"]), 15)
        for i, scenario in enumerate(compare.SCENARIOS):
            for j, flag in enumerate((False, True)):
                old = model.simulate(scenario, flag)
                new = report["development"][5 * i + j]
                self.assertEqual(old, {key: new[key] for key in old})
        for section in ("development", "holdout"):
            rows = report[section]
            for i in range(0, len(rows), 5):
                group = rows[i:i + 5]
                for row in group:
                    for field in ("scenario", "reader_arrivals", "builder_arrivals", "horizon_ms"):
                        self.assertEqual(row[field], group[0][field])
                    self.assertEqual(row["reader_arrivals"], row["reader_completed"] + row["reader_unfinished"])
                    self.assertEqual(row["builder_arrivals"], row["builder_completed"] + row["builder_unfinished"])
                    self.assertFalse(row["baseline_dominates"] and row["dominates_baseline"])
                self.assertTrue(any(row["pareto_front"] for row in group))
        print("ADAPT_COMPARE " + json.dumps(report, sort_keys=True), flush=True)

    def test_pareto_does_not_hide_tradeoffs_or_missing_values(self):
        base = dict.fromkeys(compare.METRICS, 10)
        better = dict(base, reader_p95_model_ms=9)
        self.assertTrue(compare.dominates(better, base))
        self.assertFalse(compare.dominates(base, base))
        self.assertFalse(compare.dominates(dict(better, builder_unfinished=11), base))
        self.assertFalse(compare.dominates(dict(better, reader_p99_model_ms=None), base))


if __name__ == "__main__":
    unittest.main()
