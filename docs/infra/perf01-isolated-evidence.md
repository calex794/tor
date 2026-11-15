# PERF-01 — comparable isolated TOR HTTP evidence

Tracking: [GitHub #882](https://github.com/calex794/tor/issues/882) · [TOR / PERF in Notion](https://app.notion.com/p/41832a84beec818891c1f641915a8b5b).

Status: **first measurement/reporting slice**, not an accepted capacity target and not a production benchmark. No migration, deployed setting, payment provider or production database is touched.

## Reuse instead of duplication

The existing [TOR isolated HTTP load](../../.github/workflows/tor-http-load.yml) runs k6 against an ephemeral Rust TOR and local PostgreSQL 18. It already includes HTTP read-only profiles (25/50/100/250/500 requests per second), targeted 300–500 RPS diagnostics, 10k/100k catalog fixtures, reader journeys, authenticated progress/paid test fixtures and process/PG-wait samples. It is expensive and uses a shared GitHub-hosted runner: run it only when its workflow is selected or manually dispatched.

The existing [RIL-10 route cost map](ril10-route-cost-map.md) captures route-class latencies and some SQLx client-side operation counts, but cannot claim full SQL counts for every request or PostgreSQL execution CPU. It does not automatically attach to each k6 sample. This slice **does not invent** that missing relationship.

## Evidence produced

After the existing load steps, the workflow runs the network-free parser [perf01_report.py](../../tools/perf01_report.py). It reads the already-written k6 summary exports and the step-result TSVs; it never calls TOR or PostgreSQL. Both new files are uploaded inside the existing 14-day artifact:

- `perf01-evidence.json` — exact source SHA/run ID, isolated environment, each target and actual requests/s, observed HTTP requests, p50/p95/p99, errors, dropped iterations and named-route latency distributions, plus explicit result `PASS`, `FAIL` or `INCOMPLETE`.
- `perf01-summary.md` — compact table appended to GitHub Step Summary and preserved as an artifact.
- The already-existing `diagnostic/pidstat.txt`, `diagnostic/vmstat.txt`, `diagnostic/pg-waits.tsv` and raw k6 files remain as evidence. Their existence is recorded; they are **not** converted to per-route CPU/SQL numbers without a supported association.

**Semantics:** `MEASURED` means all stage files parsed and required fields are present; it does not mean every stage passed. Failure at high rates remains a `FAIL` in the report. If the workflow aborted, the summary is missing/invalid, or required metrics are absent, mark `INCOMPLETE` and leave measurements `null`. Never fill missing values with zero. Actual HTTP RPS is not the same as a count of concurrent readers, and p95 HTTP time is not SQL execution time.

Routes use fixed, bounded k6 names; not dynamic user IDs, query parameters, cookies or URLs. The report contains no SQL text or personal identifiers.

### Standalone use on previously downloaded CI artifacts

~~~sh
python3 tools/perf01_report.py \
  --root /path/to/downloaded/tor-load \
  --sha <exact-40-character-commit-sha> \
  --run-id <github-actions-run-number> \
  --json /path/to/report/perf01-evidence.json \
  --markdown /path/to/report/perf01-summary.md
~~~

Both output paths should be private and outside the source archive if the source archive is intended as immutable evidence. The tool reads local files only.

### Fast offline checks

~~~sh
python3 -m unittest discover -s tests/architecture -p test_perf01_report.py -v
~~~

The architecture tests cover valid route quantiles, high-RPS failures, missing/corrupt summaries, invalid numbers, unexpected stage parameters, duplicated stages and provenance.

## Follow-up before closing PERF-01

1. Run and record the new report on an identical disposable dataset and precise source SHA. Preserve the raw k6 JSON, PG wait samples and runner dimensions.
2. Separately obtain trustworthy **SQLx operations per HTTP request** and SQLx client elapsed time for the four hot paths: catalog, book detail, free chapter and authenticated library. Record observed coverage; do not conflate backgrounds with the parent request.
3. Compare 10/50/100 *active-reader journeys* with controlled request mix; the current 25–500 requests/s stages are a different unit of load.
4. Obtain PostgreSQL plans/BUFFERS for actual expensive filters and measure pool occupancy. Collect any new metrics without SQL text, object keys, user IDs or private page bodies.
5. Only then set justified p95/query regression budgets and proceed to PERF-02 (book detail), PERF-03 (checkout read checks) and the remaining PERF work.

No load should be directed to the live VPS or home gateway; do not change `MemoryMax`, connection counts, PostgreSQL migration state, S3 or financial locks as part of this reporting patch.
