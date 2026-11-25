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

## ADAPT-02c: actual isolated PostgreSQL comparison (pending CI)

The existing 100k catalogue acceptance keeps its original bounded-page assertions, then reuses the same fixture and three-slot pool for six 8-second trials (baseline, age-aware, bounded-delay; reversed order in round two). No new database, workflow, runtime flags or application code. The existing integration job prints the trial evidence with --nocapture.

Each trial starts a new held generation at the same seed cursor. 170 scheduled repository feed calls (one 10/s lane throughout, three additional 10/s lanes during seconds 2–5) and 40 timed acquire/release probes share the pool with the real one-page builder. Two-second attempt limit and 500 ms pool acquisition limit remain. A common two-second reader/probe drain grace reports timeouts, errors and unstarted work; an in-flight builder page may complete after its admission horizon. Report scheduled-to-response p95/p99 and service p95 for successful feed calls, plus separately timed pool-probe p95. The probe itself adds load equally to all arms. No measurements are called HTTP latency, actual SQLx acquisition time per request, CPU/RAM, or production evidence. Workload arrival times are fixed; each lane is sequential, so late arrivals are measured from due time and unstarted work explicitly counted.

Compare real committed progress from the generation cursor and the successful page acknowledgements (a response timeout may race an already committed transaction). Record remaining materializable seeds, edge count, largest interval without acknowledged progress, pressure classifications and sleep choices. No per-seed enqueue timestamp exists, so generation age and remaining seed count are proxies, not a measured durable queue-age distribution. A fixed generation of 100k seeds means the age-aware controller can normally use the depth condition whenever a fresh low-pressure signal exists. Percentiles exclude errors/timeouts, which are reported separately. The existing read-only held-generation state, identical catalog responses, hidden sentinel denial and bounded edges remain checked. Reversed ordering helps expose, not eliminate, cache and timing confounding.

Thresholds (fresh samples within 1s, read p95 >=100ms or pool-probe p95 >=50ms high, <=50ms and <=10ms low, missing -> 5s) are illustrative, not owner-approved SLOs. No winner or runtime rollout until actual results, repetitions under an agreed reader/worker SLO, and HTTP/CPU/RAM evidence are available. No VPS/deploy. Run the existing contextual_scale ignored exact test on disposable tor_ci only.

## First measured PostgreSQL run — 2026-11-25

Evidence: PR #1248, head f1fc0d9cc1aa4c2cacd8ba46c913b22e58ab275a, merge checkout 48f9d3cb3e4564920396e937f663b50f579c0140, [integration job](https://github.com/calex794/tor/actions/runs/45636674393/job/139284032552). Existing page bound: one seed, 21 edges, 530 ms. All six trial arms passed invariant checks.

| Round | Policy | Completed / 170 | Timeout / unstarted | Scheduled p95 / p99 (us) | Service p95 (us) | Probe p95 (us) | Committed pages | Max no-progress gap (us) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| 1 | baseline | 162 | 1 / 7 | 3269649 / 3388176 | 276525 | 106410 | 9 | 1099583 |
| 1 | age-aware | 170 | 0 / 0 | 2635719 / 2727357 | 270016 | 113182 | 6 | 2803015 |
| 1 | progress-budget | 170 | 0 / 0 | 2660421 / 2844150 | 267242 | 115536 | 6 | 1846478 |
| 2 | progress-budget | 170 | 0 / 0 | 2616501 / 2690319 | 268907 | 121685 | 6 | 1977848 |
| 2 | age-aware | 170 | 0 / 0 | 2449412 / 2708742 | 262822 | 92865 | 6 | 2603540 |
| 2 | baseline | 134 | 1 / 35 | 5444355 / 5721756 | 815733 | 257174 | 9 | 1189396 |

No reader repository errors; all 240 pool probes succeeded. Adaptive policies completed 340/340 offered reads vs baseline 296/340; 12 vs 18 committed pages in two arms each. Both arms observed high and neutral pressure, no low-pressure or missing-signal decision; thus this run does not verify recovery or stale-signal behavior. The budget cap yields shorter acknowledged-progress gaps than age-aware in both rounds; age-aware has slightly lower scheduled p95 in both. Prefer budget as the first candidate for a more demanding HTTP/PG trial, not a runtime selection. Large baseline round-to-round variance, only two repetitions, sequential per-lane arrivals, and incomplete baseline reads make a production latency claim invalid. No agreed reader or worker SLO, HTTP or CPU/RAM evidence yet.

Integration and REC acceptance passed; Repository rules failed because the manual REC workflow lacked the same --nocapture flag. Preserve the exact job-parity guard, synchronize both wrappers on current main (including REUSE-11B), and run fresh CI. No tests removed or acceptance thresholds relaxed.
