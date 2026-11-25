"""ADAPT-02a: deterministic toy shared-pool experiment, not measured TOR latency."""
from collections import deque
import json
import math

TICK_MS = 50
MIN_DELAY = 5  # current busy delay 250 ms
MAX_DELAY = 100  # current idle/error delay 5 s
SLOTS = 3  # example disposable PG18 pool, not a production assertion
READER_SERVICE = 3
BUILDER_SERVICE = 8  # synthetic 400 ms service; excludes actual SQL/timeout effects


class Pacer:
    def __init__(self):
        self.delay = MIN_DELAY
        self.healthy = 0

    def after_progress(self, pressure):
        if pressure == "missing":
            self.delay = MAX_DELAY
            self.healthy = 0
        elif pressure == "high":
            self.delay = min(MAX_DELAY, 2 * self.delay)
            self.healthy = 0
        elif pressure == "low":
            self.healthy += 1
            if self.healthy >= 2:
                self.delay = max(MIN_DELAY, self.delay - MIN_DELAY)
                self.healthy = 0
        elif pressure == "neutral":
            self.healthy = 0
        else:
            raise ValueError("unknown pressure")
        return self.delay


def percentile(values, p):
    return sorted(values)[math.ceil(len(values) * p / 100) - 1] if values else None


def arrivals(scenario, t):
    """Same deterministic arrivals for baseline and adaptive; time in ticks."""
    if scenario not in ("idle", "steady", "burst_recovery", "stale", "saturation"):
        raise ValueError("unknown scenario")
    if scenario == "idle":
        return 0, 0
    high = scenario == "saturation" or (scenario in ("burst_recovery", "stale") and 200 <= t < 600)
    readers = 1 if high or t % 2 == 0 else 0
    return readers, int(t % 20 == 0)


def simulate(scenario, adaptive, ticks=1200, *, controller=None, workload=None):
    if ticks <= 0 or (workload is not None and len(workload) != ticks):
        raise ValueError("positive horizon and matching workload required")
    if controller is not None and not adaptive:
        raise ValueError("controller requires adaptive mode")
    pending = deque()  # shared FIFO connection wait
    backlog = deque()  # durable background arrivals not yet submitted
    active = []
    samples = deque()  # observed foreground wait times from past starts
    reader_latencies = []
    reader_waits = []
    reader_arrivals = reader_done = builder_arrivals = builder_done = 0
    max_age = max_pending = 0
    builder_outstanding = False
    next_attempt = 0
    delays = []
    pacer = Pacer()

    for t in range(ticks):
        if workload is None:
            readers, jobs = arrivals(scenario, t)
            missing = scenario == "stale" and 200 <= t < 600
        else:
            readers, jobs, missing = workload[t]
        # Complete tasks and observe only historical connection waits.
        while samples and samples[0][0] <= t - 20:
            samples.popleft()
        remaining = []
        for finish, kind, born, started in active:
            if finish > t:
                remaining.append((finish, kind, born, started))
                continue
            if kind == "reader":
                reader_done += 1
                reader_latencies.append((finish - born) * TICK_MS)
            else:
                builder_done += 1
                builder_outstanding = False
                if adaptive:
                    if missing:
                        pressure = "missing"
                    elif any(k == "reader" for k, _ in pending) or any(wait >= 2 for _, wait in samples):
                        pressure = "high"
                    elif all(wait <= 1 for _, wait in samples):
                        pressure = "low"
                    else:
                        pressure = "neutral"
                    if controller is None:
                        delay = pacer.after_progress(pressure)
                    else:
                        oldest_age = t - backlog[0] if backlog else 0
                        delay = controller.after_progress(pressure, oldest_age, len(backlog))
                        assert MIN_DELAY <= delay <= MAX_DELAY
                else:
                    delay = MIN_DELAY
                delays.append(delay * TICK_MS)
                next_attempt = t + delay
        active = remaining

        reader_arrivals += readers
        builder_arrivals += jobs
        pending.extend(("reader", t) for _ in range(readers))
        backlog.extend(t for _ in range(jobs))
        if not builder_outstanding and t >= next_attempt:
            if backlog:
                pending.append(("builder", backlog.popleft()))
                builder_outstanding = True
            else:
                next_attempt = t + MAX_DELAY

        while pending and len(active) < SLOTS:
            kind, born = pending.popleft()
            service = READER_SERVICE if kind == "reader" else BUILDER_SERVICE
            if kind == "reader":
                samples.append((t, t - born))
                reader_waits.append((t - born) * TICK_MS)
            active.append((t + service, kind, born, t))

        assert len(active) <= SLOTS
        assert sum(k == "builder" for k, _ in pending) + sum(k == "builder" for _, k, _, _ in active) <= 1
        oldest = list(backlog) + [born for k, born in pending if k == "builder"] + [
            born for _, k, born, _ in active if k == "builder"
        ]
        if oldest:
            max_age = max(max_age, (t - min(oldest)) * TICK_MS)
        max_pending = max(max_pending, len(pending))

    unfinished_readers = sum(k == "reader" for k, _ in pending) + sum(k == "reader" for _, k, _, _ in active)
    unfinished_builders = len(backlog) + sum(k == "builder" for k, _ in pending) + sum(k == "builder" for _, k, _, _ in active)
    assert reader_arrivals == reader_done + unfinished_readers
    assert builder_arrivals == builder_done + unfinished_builders
    return {
        "scenario": scenario, "policy": controller.name if controller is not None else ("adaptive" if adaptive else "existing_busy_idle"),
        "horizon_ms": ticks * TICK_MS, "reader_arrivals": reader_arrivals,
        "reader_completed": reader_done, "reader_unfinished": unfinished_readers,
        "reader_p95_model_ms": percentile(reader_latencies, 95),
        "reader_p99_model_ms": percentile(reader_latencies, 99),
        "pool_wait_p95_model_ms": percentile(reader_waits, 95),
        "builder_arrivals": builder_arrivals, "builder_completed": builder_done,
        "builder_unfinished": unfinished_builders, "max_builder_age_model_ms": max_age,
        "max_connection_queue": max_pending,
        "min_progress_delay_ms": min(delays, default=None),
        "max_progress_delay_ms": max(delays, default=None),
    }


def experiment():
    return {
        "evidence": "SYNTHETIC_MODEL_NOT_PRODUCTION_OR_PG_MEASUREMENTS",
        "tick_ms": TICK_MS, "pool_slots": SLOTS,
        "reader_service_ms": READER_SERVICE * TICK_MS,
        "builder_service_ms": BUILDER_SERVICE * TICK_MS,
        "results": [simulate(scenario, adaptive)
                    for scenario in ("idle", "steady", "burst_recovery", "stale", "saturation")
                    for adaptive in (False, True)],
    }


if __name__ == "__main__":
    print(json.dumps(experiment(), indent=2, sort_keys=True))
