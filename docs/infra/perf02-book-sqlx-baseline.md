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

## PERF-02b: comparable guest versus verified BASIC session

The existing S059 acceptance first captures the old anonymous-book baseline with cookie auth **disabled**. It then performs the original trace store outage/recovery acceptance. **After** S059 browser checks, it creates a random 256-bit server-session token and inserts only its SHA-256 digest, never the bearer itself, into `tor_ci.sessions`. A synthetic disposable account is created with an Argon2id-prefixed non-login test fixture; no user registration, real passwords, external traffic or funding occur. It stops the original TOR test server, restarts the **same debug binary and same PostgreSQL fixture** with `TOR_AUTH_ENABLED=1`, and checks readiness.

The tool then compares two modes on the **same book and same auth-enabled process**:

- `guest_auth_enabled`: no session cookie; render a public BASIC book
- `authenticated_basic`: synthetic valid `tor-dev-session` cookie, explicitly require real authenticated book-follow controls

For each mode it gathers **16 actually delivered S059 SQLx traces**, retaining the total request attempts (sample rate about 1/16). The existing SQLite trace store supplies task-local completed SQLx query counts and elapsed microseconds. Only route family `books`, anonymous mode labels, bounded aggregate p50/p95 values, and attempts are saved. Session bearer, digest, account ID, book ID, trace IDs, SQL and response body are not stored in this new comparison result.

The result is `report.json.perf02_auth_enabled_book_comparison` in the existing `s059-real-pg-diagnostics` artifact. It includes `p50_query_delta_reader_minus_guest` and makes the sampling limits explicit. The earlier `perf02_guest_book_sqlx_baseline` is preserved for historical comparison, but its auth-disabled mode must **not** be mechanically subtracted from authenticated measurements: the new paired comparison uses auth enabled in both arms.

**Acceptance:** S059 real PG/browser checks and full Integration CI must pass. This is observational, not proof of an optimization. A 1000-book seeded CI dataset does not establish production p95, logged-in reading load, ownership/age/rating configuration or concurrent session revocation semantics. Do not collapse fresh ACL checks merely because two queries have the same session digest; validate the security role first. User-visible code, real accounts, PostgreSQL schema, S3, migrations and deployment remain unchanged.

## PERF-02c: one snapshot for book social controls (proposed optimization)

The measured [S059 run #43591791385](https://github.com/calex794/tor/actions/runs/43591791385) found **5 SQLx completions per guest book GET, 20 for a verified reader** in the same auth-enabled process, with SQLx p95 3.252 ms vs 4.341 ms and HTTP p95 5.210 ms vs 8.014 ms. This is a real **15-query gap** in a disposable 1000-book fixture, not a production performance failure.

The follow-control rendering path in `crates/web/src/book_follow_page.rs` currently makes one **fresh server-side session lookup**, then up to four *independent* reads for book follow, owner/author controls, author identity (validated against the active B-owned membership), and creator follow. The book used by S059 has no author identity, so exactly **three** separate non-session reads are executed. They can be replaced by **one statement/snapshot** without combining authorization, subscription mutations or lifecycle writes.

PERF-02c introduces `PgCreatorFollows::book_page_follow_state`: one parameterized read-only SQL with scalar EXISTS projections over `reader_book_follows`, `books`, `book_members` and `creator_follow_periods`. The semantics match the old individual queries; the B-owned page authorization and **fresh session validation remain unchanged**. The snapshot is *for rendering only*: every mutation endpoint independently rechecks identity, actor permissions, CSRF and access. No session caching, no SQL text/user identifiers in telemetry, no new index or migration.

The existing E-05 real PG test now verifies follower/owner/creator/public/hidden and follow/unfollow parity; the existing C-15 HTTP test verifies rendering and access effects; the existing S059 operator trace runs the same paired anonymous/verified-reader measurement and requires a **median at most 18 completed SQLx operations** for the verified reader in its fixed fixture (before: 20). This is an isolated regression guard and must not be interpreted as live production SLO or applied to other role/feature configurations.

**Expected:** reader 20 → 18 SQLx operations on this fixture; guest remains 5. Real numbers must be taken from the new successful CI artifact, not assumed. Later PERF-02 slices should identify the remaining session and book-detail costs, especially active reader permission/revocation and adult/owner/paid cases, before further batching.
