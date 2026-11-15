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


## PERF-02d: one fresh A-session and follow-controls projection

This slice supersedes the initial attempt to pass a validated user id between
ratings and follow rendering. An actor cached for the duration of a GET would
miss a revocation that happens after the ratings query.

Instead, `PgCreatorFollows::book_page_follow_state_for_session` receives the
server-derived SHA-256 session digest and book id. One SELECT with a materialized
active-session viewer and owner/creator/follow predicates returns zero rows if
the session is absent, revoked, expired, or the account is inactive. The renderer
shows the guest login link in that case. The creator and ownership projection
is still B-owned and read-only; mutation endpoints keep independent A/CSRF and
Book ACL checks. No new session cache, privilege, schema, migration, or worker.

The old `book_page_follow_state(user_id,book_id)` remains available for its
existing domain tests; it is never used by HTTP to derive authority from a
client-supplied id. PG tests cover missing, valid, revoked and expired synthetic
sessions and state parity.

**Benchmark:** baseline PERF-02c was measured at 5 guest / 18 reader SQLx
completions for S059's isolated BASIC book. The new HTTP read replaces a full
`PgAccounts::active_session_user` transaction plus a separate social-state
SELECT with one social-state SELECT that verifies the current session. The exact
saving must be taken from the current S059 CI artifact, not inferred from code.
Security and latency gates remain mandatory; a cheap SQL-query count is not
proof of lower PostgreSQL CPU or lower concurrency p95. Authenticated adult and
owner scenarios require independent safety checks.

Existing bounded session_actor checks in the other HTTP paths are unchanged.
This new direct read cannot be used for privileged mutations, private chapter
delivery, entitlement grants, or financial actions.


## PERF-02e: one access-gated star + like aggregate

Both F-05's `PgBookStars::visible_summary_for_actor` and C's
`PgBookLikes::direct_view_summary/direct_view_count` check a current
public/active Book and the same actor-specific BASIC/18+/21+/administrator
visibility gate. Direct Book GET is allowed to render during
`pending_deletion`, while the standalone like/rating mutation gates still
deny writes and public discovery.

`PgBookStars::book_page_summary_for_actor` now returns the two star vote
aggregates **and** a scalar like count from one SQL statement/snapshot, with
one age/visibility predicate. `book_rating_context::for_book` continues
validating the A session first, then performs one aggregate SELECT instead
of separate star SELECT and likes SELECT. Ordinary F-05 callers retain
`visible_summary_for_actor`; its internal query uses a conditional CASE
with `include_likes=false` so they do not scan the likes table.

No favorite flags or personal Like rows are returned, no cached Book
authorization is introduced, and all POST mutations retain their
independent current-actor checks. Real PostgreSQL tests cover basic
guest/reader parity, pending-deletion direct reads, 18+ revocation and
active moderation hold, alongside existing F-05 and Book Like browser tests.

**Acceptance:** S059 real PG HTTP query trace count and latency, full
PostgreSQL integration/browser/security suites, and repository rules. With
PERF-02c measured at 18 SQLx operations for an authenticated BASIC book,
this patch removes one separate like-count query on the same path in code.
An actual new number must come from the CI trace artifact. There is no
production performance claim, migration, pool increase, live DB change
or deployment.


## PERF-02g: successful SQLx pool acquisition duration in the same Book request

The PERF-02f release k6 suite passed up to 200 GET/s with no dropped iterations
on a disposable PostgreSQL 18 runner, but `pg_stat_activity` cannot observe
tasks waiting in the application-side SQLx connection pool.

SQLx 0.8.6 provides an opt-in numeric `sqlx::pool::acquire` tracing event
(the upstream field is spelled `aquired_after_secs`). PERF-02g enables it
**only** when all three are true: `GITHUB_ACTIONS=true`,
`TOR_CI_PERF02_POOL_TIMING=1`, and the database URL ends in
`@127.0.0.1:5432/tor_ci`. No live connections, pool size, timeouts, query
executors, business authorization or PostgreSQL schema are changed.

The existing RIL task-local subscriber now independently counts successful
pool acquisitions, their total elapsed microseconds and worst acquisition
duration for each HTTP request. The original SQLx query count/timing and
bounded query-span evidence remain unchanged. Approximately one in sixteen Book
requests is selected using SplitMix64 over a process-local sequence number, not
fixed periodic sampling. This avoids phase-lock bias between equal-rate guest
and authenticated k6 scenarios. Each selected request emits a numeric line
without route IDs, SQL text,
session cookies, IP addresses, user IDs, book IDs or trace IDs. The benchmark
collects stage-labeled summaries of these values, along with existing k6
guest/reader p50/p95/p99, PG activity and pidstat artifacts. The report
also groups selected requests by observed numeric SQLx query count to expose
cohort imbalance; query counts are not actor or authorization labels.

**Important scope:** SQLx acquisition duration includes time waiting for a
semaphore, checking idle connections, and establishing new connections. It is
NOT pure application pool queue wait, PostgreSQL CPU, lock-wait time or a
per-query DB execution duration. SQLx emits these events after successful
acquisitions, not after timeouts. Stage samples include the few GETs used by
k6 `setup()` to verify active sessions; they are diagnostic and **not** a
paired request-level A/B study. `tools/perf02_pool_evidence.py` fails a
nominally successful k6 stage if there are insufficient real numeric samples;
missing acquisition evidence must never be reported as zero latency. k6
--summary-export metrics may expose p50/p95/p99 directly or nested in values;
the parser accepts both formats and validates the numeric statistics.

Acceptance requires Repository rules, full Integration CI and isolated HTTP
load on the current HEAD, including unit checks of the actual SQLx 0.8.6
field name and the numeric-only report parser. Follow-up work must repeat
representative adult/owner/pending-deletion workloads and measure wider
competition before closing PERF-02. No deployment, new index or migration.


## PERF-02h: mixed real-user journeys under increasing load

PERF-252 already proves 25 complete journeys per second for 60 seconds,
against disposable `tor_ci`: 13 persisted free-progress/comment journeys,
10 public catalogue/book/reader journeys, and two real paid purchases per
second. Each purchase is verified against actual wallet deductions, immutable
purchase facts and pinned library rights, without a payment provider.

PERF-02h adds two *non-gating capacity probes* to that same isolated server
and PostgreSQL 18 schema, reusing the accounts from the mandatory 25/s run:

| Stage | Duration | Writer journeys/s | Public journeys/s | Paid journeys/s | Paid chapter ordinals |
| --- | ---: | ---: | ---: | ---: | --- |
| Required baseline: 25 journeys/s | 60 s | 13 | 10 | 2 | 1-121 |
| Capacity: 50 journeys/s | 15 s | 26 | 20 | 4 | 122-182 |
| Capacity: 100 journeys/s | 15 s | 52 | 40 | 8 | 183-303 |

A *journey* contains multiple real HTTP requests and is not the same as
one GET/s. New capacity stages log into existing readers rather than
re-registering duplicate accounts. The paid ordinals cannot overlap,
so replay/idempotency cannot masquerade as additional purchases. Writer
progress remains conditional on the last persisted revision and free comments
are checked in the rendered book discussion. The final database assertion
requires `purchase facts = wallet purchase entries = pinned library rights`
and verifies the exact remaining total balance. No private paid version is
exposed to a guest or to a non-buyer.

Custom k6 Trends capture p50/p95/p99 for public Book GET, free-progress
write and paid checkout. `tools/perf02h_mixed_evidence.py` writes strictly
numeric summaries of all three stages and rejects a "PASS" missing actual
metrics. The existing 25/s baseline still gates CI. If a 50/100 probe fails,
the report must preserve that **FAIL**, never present it as zero latency or
silently claim additional supported capacity. CPU samples are retained in
the short-lived isolated CI artifact.

Limitations: each stage uses a small test fixture, short-lived GH runner
and a fixed SQLx pool of three connections; HTTP p95 is not a controlled
before/after comparison of PERF-02 optimizations. More throughput is not
automatically more distinct concurrent readers, nor is it a live VPS
capacity guarantee. Owner/18+/21+/revoked sessions still need independent
role-specific acceptance before closing parent #888.

No migrations, production traffic, working VPS/PG/S3 changes, pool resizing,
wallet credit to real accounts, or deployment.


## PERF-02h: duration-boundary fixture isolation

The first real mixed-load run passed 25 journeys/s but reported one purchase
failure at 50/s and two at 100/s despite zero HTTP failures or dropped
iterations. k6 scheduled 121, 61 and 121 buyer iterations at the duration
boundaries. The original ranges (1..120, 121..180, 181..300) therefore overlapped:
the first purchase in the next stage was an idempotent replay, and the final
stage also exhausted its fixture. This was not evidence of PostgreSQL saturation.

Each stage now owns its nominal buyer count plus one bounded endpoint reserve:

| Journeys/s | Paid editions reserved | Maximum buyer iterations |
| ---: | --- | ---: |
| 25 | 1..121 | 121 |
| 50 | 122..182 | 61 |
| 100 | 183..303 | 121 |

The disposable fixture seeds 303 editions. An iteration beyond its own stage
capacity still fails before HTTP; a replay instead of a new purchase still
fails. No scheduled iteration is silently discarded and no failure threshold
is relaxed. Wallet, purchase and entitlement postconditions remain exact.

An executable Node/VM regression invokes the actual k6 paidReader function
across all three sequential profiles with both nominal and extra boundary
iterations. It reproduces the old overlap, rejects unexpected exhaustion,
and preserves the rejection of an HTTP-200 replay as a new purchase. This
isolated harness does not replace the real PostgreSQL/HTTP load acceptance.
