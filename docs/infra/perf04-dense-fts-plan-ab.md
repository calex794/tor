# PERF-04b — controlled FTS planner A/B (isolated PostgreSQL)

Tracking: [PERF-04 #884](https://github.com/calex794/tor/issues/884), [PERF line in Notion](https://app.notion.com/p/41832a84beec818891c1f641915a8b5b).

## Measured starting point

From [TOR isolated HTTP load #43583491113](https://github.com/calex794/tor/actions/runs/43583491113) on a disposable PostgreSQL 18 with 10,000 public books:

| Query | Actual FTS matches | PostgreSQL estimate | Plan | SQL execution | Shared hits |
| --- | ---: | ---: | --- | ---: | ---: |
| `Load` (dense) | 10,000 | 50 | normalized GIN bitmap scan, heap scan, top-N sort | 3.068 ms | 438 |
| `9999` (rare) | 1 | 50 | same normalized GIN bitmap plan | 0.585 ms | 102 |

The normalized GIN `books_public_discovery_terms_nfkc_idx` **already exists** and was used in both plans. It is defined by migration `202610270005_f03_normalized_discovery.sql`. Earlier speculation that a matching index was missing was **incorrect**.

The SQL search latency varied across three GitHub-hosted k6 runs (dense 500 RPS p95 ~1,629, ~43, ~5.6 ms). A single run does not justify altering production query plans, indexes or VPS sizing.

## Diagnostic question

The query requests the first 20 rows plus one cursor probe in ascending `id` order. When almost all public books match the query, a sequentially **ordered index scan** may find the first 21 matches much earlier than a GIN bitmap scan that materializes 10,000 matches and sorts them. Conversely, forcing ordered scans for rare terms could be far slower than GIN. The correct choice must be measured in both cases.

The same exact prepared SQL from `ASCENDING` in `crates/search/src/repositories/catalog.rs` is executed on disposable `tor_ci` under two planner configurations, without changing query semantics:

- **Default:** regular PostgreSQL cost-based planning, normal GIN/bitmap options.
- **Forced ordered-access experiment:** `SET LOCAL enable_bitmapscan=off` and `SET LOCAL enable_seqscan=off` inside `BEGIN READ ONLY`, to compare an available index-ordered path. This is a *diagnostic hint*, **not** a suggested server setting or production query change.

Each is tested with dense `Load` and rare `9999`, with `EXPLAIN (ANALYZE, BUFFERS, SETTINGS)`, `LIMIT 21`, the original visibility/age ACL constraints, and identical `NFKC/lower/ё-е` normalization. Both plans are generated **after** `ANALYZE books` on 10k and 100k book fixtures.

## Evidence and safety

The existing `.github/workflows/tor-http-load.yml` stores outputs at:

- `perf04-fts-10k-ab.txt`
- `dense100k/perf04-fts-100k-ab.txt`

Both files are included in the existing 14-day `tor-http-load-up-to-500rps` artifact. No new runner or service is added.

The 100k profile reuses `tests/load/catalog_dense_100k_ci.sh`, whose mandatory guard checks that the workflow is a PR, host is `127.0.0.1`, database is `tor_ci`, and the connection role is `tor_app`. The 10k profile uses the same isolated CI PostgreSQL setup. The SQL script contains no DML, DDL or global planner settings, and rolls back the read-only transaction. A 15-second per-statement timeout avoids hanging the runner on a deliberately bad alternative plan.

`tests/architecture/test_perf04_search_plan.py` checks for read-only scoping, four EXPLAIN variants, matching runtime FTS/ACL expressions, and absence of DML/DDL. **A passing architectural test does not replace examining the real PostgreSQL plans.**

## What can and cannot be concluded

- **Yes:** Which access path the planner selects in each configuration, rows actually visited, cache hits/reads, sort work, and measured isolated SQL execution times.
- **No:** Whether the forced setting should be applied in production, how 100 concurrent readers behave on 1-vCPU VPS, or whether the original 1.6-second k6 outlier is a systemic user-facing problem.
- **Next:** If the A/B suggests a robust win, implement a separate semantically safe query-level option and re-run search Unicode, private/age ACL, cursor, 10k/100k, PostgreSQL concurrency and HTTP p95 tests before any merge. Do not alter indexes without evidence and migration review.

No deployment, production PostgreSQL, real S3, financial data, timer or migration is touched by this diagnostic.
