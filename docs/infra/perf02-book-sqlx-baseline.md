# PERF-02a — SQLx baseline of an anonymous book detail (isolated S059)

[PERF-02 #888](https://github.com/calex794/tor/issues/888) · [Notion PERF line](https://app.notion.com/p/41832a84beec818891c1f641915a8b5b) · [PERF-01 #882](https://github.com/calex794/tor/issues/882).

## Existing infrastructure — no new tracer

The project already implements numeric-only HTTP task-local SQLx observation in `crates/web/src/ril/sqlx_observer.rs`. `observability::finish_trace` delivers a bounded sample into the existing S059 writer/SQLite archive; `tools/operator_trace_pg_smoke.py` already performs real PostgreSQL→HTTP→SQLx→SQLite acceptance. This slice extends that **existing** disposable-CI test; it does not add another server, trace exporter, public route, persistence schema, or production setting.

## New evidence

On the fixture made inside `operator_trace_pg_smoke.py` (1000 `public/basic` books), a fixed anonymous `GET /books/{book_id}` is issued with no login cookies. The book ID is obtained from the disposable PostgreSQL; neither the ID nor the book body is recorded in the report.

The trace publisher samples approximately 1/16 requests. The test collects **16 actual delivered sample traces** with server-issued trace IDs, verifies `sqlx_known=1`, nonzero completed SQLx operations, and records:

- `sampled_requests` and `total_http_requests` (the sampling denominator; no fabricated full coverage)
- p50/p95 **completed SQLx operations per traced HTTP request**
- p50/p95 client-observed SQLx elapsed microseconds per traced request
- p95 observed HTTP milliseconds for those same sampled requests
- fixed `route_family=books`, `mode=anonymous_basic`, `sqlx_known_fraction=1.0` **within the sampled subset only**.

These values become `report.json.perf02_guest_book_sqlx_baseline` in the existing [S059 acceptance artifact](../../.github/workflows/tor-integration.yml). The script asserts a real 200 page with the seeded title, not an empty HTTP 200 or mocked SQL count. It refuses execution outside a GitHub Actions environment with application role `tor_app` against local disposable PostgreSQL `127.0.0.1:5432/tor_ci`.

## What this does not prove

Sampling is not a count of all user requests or unique people. The correlation is limited to completed SQLx spans in the HTTP Tokio task; spawned/background business tasks and server CPU are excluded. Shared GitHub runner debug binaries are not performance proxies for the 1 GB live VPS or a home server. This anonymous fixed fixture does not cover logged-in readers, active/delayed session revocation, adults/pro_adults, author controls, purchased copies, editorial/SEO/CDN flags or post-payment concurrency.

**Do not set a new p95 or queries/request release threshold from one short debug run.** PERF-02 remains open until comparable route variants, multiple runs, and appropriate security/PG/reader acceptance are available. Query batching or deduplicated identity checks require separate code review and post-change measurement.
