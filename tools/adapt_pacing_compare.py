"""ADAPT-02b: compare offline policies on fixed training and held-out traces."""
import json
import random

from adapt_pacing_model import MIN_DELAY, MAX_DELAY, TICK_MS, simulate

SCENARIOS = ("idle", "steady", "burst_recovery", "stale", "saturation")
HOLDOUTS = ("short_bursts", "repeated_pressure", "seeded_mixed")
MODES = ("fast_recovery", "age_aware", "progress_budget")
HORIZON = 1200
AGE_TICKS = 160  # illustrative 8 s, not a production SLA
DEPTH = 8
BUDGET_DELAY = 20  # illustrative 1 s between successful attempts, not a guarantee
SEED = 260025
METRICS = ("reader_p95_model_ms", "reader_p99_model_ms", "reader_unfinished",
           "builder_unfinished", "max_builder_age_model_ms")


class Candidate:
    def __init__(self, mode):
        if mode not in MODES:
            raise ValueError("unknown mode")
        self.name = mode
        self.delay = MIN_DELAY
        self.healthy = 0

    def after_progress(self, pressure, oldest_age, depth):
        if pressure == "missing":
            self.delay, self.healthy = MAX_DELAY, 0
        elif pressure == "high":
            # The budget candidate caps fresh-telemetry delay growth. It never
            # speeds up on a high signal, including after missing telemetry.
            ceiling = max(self.delay, BUDGET_DELAY) if self.name == "progress_budget" else MAX_DELAY
            self.delay, self.healthy = min(ceiling, 2 * self.delay), 0
        elif pressure == "low":
            self.healthy += 1
            urgent = (oldest_age >= AGE_TICKS or depth >= DEPTH)
            if self.name in ("age_aware", "progress_budget") and urgent:
                self.delay, self.healthy = MIN_DELAY, 0
            elif self.healthy >= 2:
                self.delay, self.healthy = max(MIN_DELAY, self.delay // 2), 0
        elif pressure == "neutral":
            self.healthy = 0
        else:
            raise ValueError("unknown pressure")
        return self.delay


def holdout(name):
    """Fixed independent arrival traces, never exposed to the controller."""
    if name not in HOLDOUTS:
        raise ValueError("unknown holdout")
    rng = random.Random(SEED)
    result = []
    for t in range(HORIZON):
        if name == "short_bursts":
            readers = 1 if t % 240 < 40 else int(t % 3 != 0)
            jobs, missing = int(t % 24 == 0), False
        elif name == "repeated_pressure":
            readers = 1 if t % 300 < 150 else int(t % 2 == 0)
            jobs, missing = int(t % 16 == 0), 650 <= t < 710
        else:
            # Occasionally two arrivals in one tick; independent queue bursts.
            readers = int(rng.random() < (0.9 if 250 <= t < 750 else 0.5)) + int(rng.random() < 0.08)
            jobs, missing = int(rng.random() < 0.055), 800 <= t < 900
        result.append((readers, jobs, missing))
    return tuple(result)


def dominates(left, right):
    """Strict Pareto comparison, with no invented conversion between objectives."""
    pairs = [(left[key], right[key]) for key in METRICS]
    if any((a is None) != (b is None) for a, b in pairs):
        return False
    values = [(a, b) for a, b in pairs if a is not None]
    return all(a <= b for a, b in values) and any(a < b for a, b in values)


def compare(scenario, workload=None):
    rows = [simulate(scenario, flag, HORIZON, workload=workload) for flag in (False, True)]
    rows.extend(simulate(scenario, True, HORIZON, controller=Candidate(mode), workload=workload)
                for mode in MODES)
    baseline = rows[0]
    for row in rows:
        row["baseline_dominates"] = dominates(baseline, row)
        row["dominates_baseline"] = dominates(row, baseline)
        row["pareto_front"] = not any(dominates(other, row) for other in rows)
    return rows


def experiment():
    return {
        "evidence": "SYNTHETIC_MODEL_NOT_PRODUCTION_OR_PG_MEASUREMENTS",
        "holdout_seed": SEED, "horizon_ms": HORIZON * TICK_MS,
        "objectives_minimized": METRICS,
        "development": [row for name in SCENARIOS for row in compare(name)],
        "holdout": [row for name in HOLDOUTS for row in compare(name, holdout(name))],
    }


if __name__ == "__main__":
    print(json.dumps(experiment(), indent=2, sort_keys=True))
