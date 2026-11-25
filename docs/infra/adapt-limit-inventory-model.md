# ADAPT-00 / ADAPT-02a — offline evidence

Parent #1244; work item #1245; supports F-07/S044 and PERF-01/07. Base: `fa9b406b0e8ec54640671782e0e97561595315cd`.

## Reproduce

From a clean checkout:

```sh
python3 tools/adapt_limit_inventory.py --output /tmp/tor-limit-candidates.json
python3 tools/adapt_pacing_model.py
python3 -m unittest discover -s tests/architecture -p test_adapt_lab.py -v
```

Existing Repository rules test discovery runs these tests; no new workflow or service. Logs contain inventory counts and modeled results, not source lines or secret values. Full JSON is written only by the requested local CLI output. Compare the same commit; the scanner explicitly reports tracked dirty state, scope, extension coverage and skipped files. Review each candidate before classifying.

## Limits of the scanner

This is a line-oriented lexical search of tracked source and configuration files, including test/tool references. It intentionally produces false positives (comments, capacity allocations, test constants, SQL probes) and misses some multiline expressions, unnamed arithmetic, limits expressed in other syntax, runtime env overrides, and documentation-only contracts. It is not a Rust or SQL parser and not a complete semantic audit. Each match is unreviewed; no limit is automatically relaxed. Source coordinates and matched known identifier names only, no whole source lines.

Classify as hard safety/contract, operator configuration, configurable product policy, or adaptive performance target. Record units, ownership, bounds, existing tests and dependencies in #1245. Live VPS values remain unknown. Do not change financial, access, lease/fencing or retention semantics.

## Pacing experiment

Current contextual worker uses 250 ms after progress, 5 s after idle/error, with a 2 s per-attempt deadline. The model preserves the two sleep bounds and compares this baseline with multiplicative increase of delay under pressure, additive recovery after two healthy observations, and 5 s fallback for missing signals. The idle path stays at 5 s.

The deterministic model uses one FIFO pool of three slots, 50 ms ticks, synthetic reader service 150 ms and builder service 400 ms, one outstanding builder attempt, and identical arrivals within each scenario. High pressure is an observed foreground queue or recent waits >=100 ms; low is recent waits <=50 ms with no queued reader, neutral breaks the recovery streak. Stale telemetry is explicitly injected. These are experiment parameters, not approved production thresholds. There is no real database, S3, reader request, CPU/RAM measurement, or simulation of actual query cancellation/deadline/error handling. The modeled reader latency is connection wait plus synthetic service, not HTTP latency. Percentiles are over completed reader requests, pool wait over started requests; unfinished counts are reported separately. No samples are null, not zero.

Compare reader percentiles and unfinished requests against work completed, remaining work and maximum modeled age. Slowing the builder can protect foreground traffic at the expense of freshness; a 5 s sleep ceiling is not a guarantee against starvation. Synthetic outcomes can reject a candidate but cannot establish runtime benefit or approve deployment.

## Next gate

Review source candidates, replay measured pressure with PERF-01/RIL, calibrate metrics using disposable PG18 and the existing 100k contextual scale fixture, measure reader p95/p99/errors/pool wait plus builder completion/queue age, prove correct progress and visibility, and decide if any runtime change is justified. No pool resizing, new production flags, migrations, deployment or VPS work in this PR.

## ADAPT-02b: compare before proposing runtime work

Run `python3 tools/adapt_pacing_compare.py` and the existing architecture test discovery. The five policies share one simulator and exact arrival traces. Three preselected additional synthetic traces (short bursts, recurring pressure, seeded mixed arrivals with missing telemetry; seed 260025) are separate from the five initial cases. No future arrivals are available to a controller. The candidates preserve the 250 ms to 5 s sleep envelope. Missing telemetry yields 5 s; fresh high pressure never shortens a previous delay. Fast recovery halves delay after two healthy observations. Age-aware recovery resets to the minimum on a low signal with >=8 s age or >=8 queued jobs; the progress-budget variant additionally caps delay growth at 1 s under fresh telemetry, unless the previous delay already exceeds that cap (e.g. after missing signals). These illustrative thresholds are not approved production limits or a guaranteed completion rate.

All results retain unfinished reader and builder counts; a Pareto comparison minimizes reader p95, p99, unfinished readers, unfinished builders and oldest unfinished builder age without an invented exchange rate. No policy dominates the baseline on the full objective set in the evaluated non-idle scenarios. In the burst case baseline / v1 / fast / age-aware / budget yield p95 1850/950/950/950/1450 ms, completed builder tasks 55/20/38/56/59 out of 60 and maximum unfinished age 11350/39950/28250/18750/14650 ms. On recurring pressure and seeded mixed holdouts all candidates accumulate more old work than baseline. Continuous saturation has insufficient spare pool capacity to guarantee both reader and builder objectives.

Keep age-aware and bounded-delay variants as candidates, not a production decision. Next measure real pool wait, queue age, CPU/RAM, HTTP p95/p99/errors, and completed generation progress on the existing disposable PG18 100k fixture. Use baseline-derived acceptance limits; replay independent measured traces. No runtime configuration, rights, deployment or database changes.
