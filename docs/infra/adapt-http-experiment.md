# ADAPT-02d — first isolated loopback HTTP experiment

Refs #1244/#1245. Base 78cdaab55a26b5f5d802b3db9b6f3b6dc9c57c20. Standalone ignored test, actual Axum recommendation GET over TCP loopback and PG18. No application code changes or production configuration. Same test-only pacing policy and classifier extracted from ADAPT-02c; real PgCatalog three-connection pool with normal runtime acquisition timeout, shared by reader and builder. The two-second builder deadline and 250ms–5s sleep envelope stay intact.

Four 24s arms (baseline / progress-budget, reversed round two). 6s at 2/s, 8s at 30/s, 10s at 2/s, 272 independent due-time offers per arm. Five-second drain, per-phase p95/p99 (successful reads only), HTTP errors by status, transport, timeouts and unstarted explicitly counted. Same-pool probe adds identical load; it is not each HTTP request's SQLx wait. CPU ticks and RSS kB sampled each second from combined client/router/worker process; not separate TOR/PG CPU. Missing samples are None. Held generation, expected feed, hidden sentinel denial, cursor progress and edge bound are checked. No live data. Experimental thresholds, two repeats, no forced stale signal in this first trace; falling offered load does not necessarily imply observed low pressure. This is not a full production process or capacity test. Future wider k6 runs should reuse existing PERF-01/pidstat/vmstat. No accepted runtime algorithm or SLO. Current REC CI lane only; historical lane unchanged. No new workflow, migration, VPS or deployment.

First measurements are recorded below. This remains an experiment, not an accepted runtime policy.

## First CI correction
Initial head 40b83aa7: Integration 45647567755 and REC 45647567251 stopped at rustfmt; Repository rules 45647567248 reported 2 subtest failures (both REC wrappers had two current-only guards instead of the required one). Apply the exact emitted rustfmt hunks; retain both PG and HTTP cargo test invocations in one existing current-only step in both wrappers. No architecture test, workload, assertion or controller changes. Fresh CI required; HTTP measurements not yet available.

## Measured HTTP and PostgreSQL replication — 2026-11-25

Exact head fc55ea9077071dc648571da121dfa3587ee9c326; merge checkout 1234e0b751f935ae4cd9dc7e98b1f875dbdfc448; PG18 current-lane [job 139325837428](https://github.com/calex794/tor/actions/runs/45648419476/job/139325837428), SUCCESS. The real loopback test finished in 150.87 s. All arms had 272/272 HTTP successes, no HTTP/transport errors, timeouts or unstarted requests. All 480 shared-pool probes succeeded. Units below are milliseconds rounded from successful-service measurements; phase attribution is by scheduled offer, not completion. Pages are committed cursor visits, including any late acknowledgments; gaps are elapsed time including the terminal interval.

| Round / policy | Normal HTTP p95 ms | Burst HTTP p95 / p99 ms | Recovery HTTP p95 ms | Probe p95 ms | Pages | Errors / timeouts | Longest gap ms | Page completions (normal / burst / recovery*) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 Baseline | 373 | 3836 / 3947 | 3538 | 902 | 1 | 3 / 1 | 23203 | 1 / 0 / 0 |
| 1 ProgressBudget | 373 | 3488 / 3612 | 3214 | 923 | 3 | 4 / 0 | 20776 | 3 / 0 / 0 |
| 2 ProgressBudget | 143 | 3440 / 3525 | 3065 | 798 | 4 | 3 / 0 | 20296 | 3 / 0 / 1 |
| 2 Baseline | 93 | 3481 / 3554 | 3114 | 744 | 11 | 1 / 1 | 13907 | 6 / 0 / 5 |

*The round-2 ProgressBudget recovery completion was at 24.108 s, after the nominal 24 s horizon. No adaptive arm demonstrated a Low pressure decision; no controlled missing signal was injected. Recovery is not demonstrated. Cumulative process CPU tick deltas over sample horizon: baseline 202/106; budget 131/100. RSS peak kB: baseline 98736/99904; budget 99552/99680. These are combined test client/router/worker process readings with warm-order carryover, not separate server/PG costs or statistically significant resource improvements.

The repeat of ADAPT-02c in the same job contradicts a strong interpretation of the first run:

| Policy | Successes / 340 | Reader timeouts | Unstarted | Pages | Builder errors |
|---|---:|---:|---:|---:|---:|
| Baseline | 318 | 2 | 20 | 3 | 3 |
| AgeAware | 316 | 2 | 22 | 3 | 3 |
| ProgressBudget | 307 | 2 | 31 | 4 | 2 |

Earlier ADAPT-02c had 340/340 for both adaptive policies versus 296/340 baseline. Workload is unchanged but runner variation and builder failures are material; no production pacing choice is supported. The acceptance tests prove correctness and accounting under the measured conditions, not a performance win.

### Diagnosed blocker: bounded candidate materialization

The PostgreSQL service log in job 139325837428 includes repeated "canceling statement due to statement timeout" for the actual content candidate ranking INSERT (WITH candidates, per-tier row_number, score from genre/tag intersections, lexical id tie break). The source is crates/recommendations/src/repositories/feedback/sources.rs, book_sources, called by contextual/build.rs with SET LOCAL statement_timeout='750ms'. This is not merely pool wait or a pacing decision. Existing logs do not attribute every generic application error/outer timeout to the same SQL failure. Do not raise the 750 ms statement deadline, 2 s tick deadline or pool size to hide the issue.

Next: isolate exact baseline candidate SELECT/INSERT plan on 100k sparse/dense cohorts with EXPLAIN (ANALYZE, BUFFERS) on disposable data; distinguish candidate enumeration, distinct intersections, partition ranking and write cost. Compare any alternative to an independent exact oracle including duplicate metadata, caps, tier partitions, tie order by id::text, hidden/held/deleted rows, seed exclusion, manual edge channel, committed cursor and rollback. Preserve source coefficients, policy, bounds and visibility. Only then repeat PG + HTTP comparison with independent runner repetitions, separately label attempts, timeouts and observed pressure/recovery. No forced deadline relaxation or production rollout. Consider separate diagnostic substage ADAPT-02e / ALG-03 before ADAPT-03.

### Second CI correction

At fc55ea90, Repository rules and REC feedback acceptance succeeded; Integration failed two Clippy lanes because the shared test-only AgeAware variant was not instantiated by the HTTP test binary (-D dead-code). Add a non-ignored common policy transition unit test using all three variants; retain -D warnings and both original measured traces. Fresh CI required.
