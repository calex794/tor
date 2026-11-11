# FLOW-09 offline review supplement

Status: **UNCOMPILED / UNVERIFIED**. This is review and recovery material.
No PostgreSQL, HTTP, browser, formatter or compiler has run on this new package.
The original recovery commit c504492b4746ebbce9dac79020ff6f5afcaa69d5 is preserved
unchanged. Files in this supplement are additional prepared inputs, not accepted
production changes or reconstructed missing integration source.

## Reviewed boundary and findings

The review covers the eight contents files and patches 01–04 of that immutable
checkpoint. The F observation-health base and H HTML-health work must still be
recovered from their own known commits; no substitute is constructed here.

### 1. Future database timestamps could remain fresh — fix prepared

The original report uses greatest(0, now()-finished_at). After a backward database
clock step, a previously recorded future check can look age zero until time catches
up. Save also checks only the lower age bound and could preserve its success
baseline. A second boundary error comes from accepting floor(age)<=180, which
allows a check just over 180 seconds old.

05-static-review-fixes.patch uses both timestamp bounds for baseline continuation
and exact timestamp bounds for report freshness. A future or >180-second snapshot
is unknown. Display age is absent for future timestamps. A new 200 can establish a
fresh baseline; a first 500 after the clock/gap reset remains unknown.

Prepared regression: set both stored times 180.5 seconds in the past, then one
hour in the future. In each case report.checked_targets()==0, and a subsequent
first HTTP 500 cannot become Lost from the invalid prior baseline.

### 2. Old-origin success leaked into the new source's explanation — fix prepared

The old report already made current status unknown when the configured origin
changed, but still displayed last_success_at from the old origin. This does not
change counts, yet it supplies evidence from a different source.

05-static-review-fixes.patch suppresses that historical timestamp unless the
completed origin matches the requested configured origin. The save path already
resets baseline on an origin change. The prepared test checks the report before
the new-origin round, when the old implementation would still expose that date.

### 3. Four slots were counted but their identities were not checked — fix prepared

Save checked length and pairwise result/lease equality, but accepted reordered or
duplicated lease slots. The production claimant always produces 1,2,3,4, and no
HTTP caller can supply these values, so this is a boundary-invariant defect rather
than an exposed arbitrary-URL API.

05-static-review-fixes.patch requires exactly Home, Catalog, Book, FreeChapter in
that order. The prepared test reorders a valid claimed lease and expects Invalid
before any snapshot mutation; the original valid lease remains usable.

### 4. Scope changes do not prove restoration of an earlier URL — wording fixed

A confirmed lost book A may lawfully become hidden/deleted/paid. If no eligible
replacement exists, that slot becomes NoTarget. The currently checked set can then
contain only home and catalog. Complete 200 evidence for those current targets
can close the current-sample condition; it does not prove that A recovered.

This meaning was explicitly accepted by integration after independent review.
No silent scope-fingerprint extension is introduced. New eligible B, a changed
revision, or changed origin remains Unknown until a new 200 establishes evidence.
Unknown or stale plus zero never yields recovery.

06-current-sample-text.patch uses the exact accepted sentence in current healthy
state, current source evidence and the public-page history entry:

> В текущей проверяемой выборке нет подтверждённых потерь. Восстановление ранее проверенных URL не утверждается

It also removes the previous-period arrow from PublicPageLoss history; this rule
describes the latest current sample, not equal audience periods or restoration
of every URL previously seen.

## Source invariants that survived static review

- Source paths are generated from the fixed four slot types and positive B/C IDs.
  No reader-supplied URL, cookie, account, IP, session, text or arbitrary path is
  accepted by the HTTP interface. The configured origin validator rejects userinfo,
  path/query/fragment, and non-loopback plain HTTP.
- reqwest disables proxies and redirects, sends HEAD with the known bot UA and no
  credentials, limits connect to 300ms/request to 750ms, and bounds the round to
  five seconds. Each two-element chunk has at most two concurrent requests.
- Claim releases its transaction before network requests. Current checks release
  their read-only transactions before HEAD. Pending patch03 adds the immediate
  post-HEAD eligibility/revision check. Save checks again under brief B/C SHARE
  locks after all network work; no connection is intentionally held across HTTP.
- Candidate policy requires an anonymous basic public active book, no hold/delete
  or pending deletion. A free chapter also requires its current publication and
  matching immutable version, current release time, public/free/no hold/no delete.
  Paid/private/saved reader targets are not candidates.
- Fingerprints include the current book/chapter/publication xmin and version ID.
  A hold→unhold or revision replacement resets evidence even if the URL is again
  the same. Current-source GET rechecks eligibility and never renders an old
  hidden target link. The selection is a sample, not a URL inventory.
- Singleton epoch/lease fencing prevents a stale worker overwriting a newer
  claimed round. Save rechecks the live lease before commit and rolls back the
  whole snapshot on failure. At most four slot rows and one singleton exist.
- First non-200, network failure, missing baseline, changed target/origin/revision,
  stale/future source evidence remain Unknown. A matching prior 200 followed by
  another actual HTTP status can be Lost; no network error is recast as HTTP 500.
- Genre classes aggregate every distinct nonempty exact genre before any list
  LIMIT. Duplicate labels on one book count once. Books are not multiplied in the
  all-book total. The bounded first20 low-label list is explanatory, not the
  denominator. All unknown or partial unknown zero is not Healthy.
- Closed exposure counts, complete genre summary and health use one RR read-only
  transaction and the same anchor. Ordinary current-calendar reports keep their
  prior scope. No broader default or new individual history is introduced.

These are static review conclusions, not runtime proof.

## Prepared test source inventory

The prepared-tests directory maps to these eventual repository paths:

| Prepared file | Restore to | Intended evidence |
| --- | --- | --- |
| flow_genres_pg.rs | crates/recommendations/tests/flow_genres_pg.rs | all 103 genres beyond detail100; duplicate/blank labels; immutable snapshot across concurrent count+health commit |
| public_pages_pg.rs | crates/search/tests/public_pages_pg.rs | initial/missing/network/origin/stale/future distinctions; public policy/revision transitions; lease fencing, read-only source and lock budget |
| public_pages_pg/fixtures.rs | crates/search/tests/public_pages_pg/fixtures.rs | isolated current-schema clones and guarded tor_ci URLs |
| public_pages_pg/head.rs | crates/search/tests/public_pages_pg/head.rs | real Axum HTTP server, only HEAD/fixed targets, <=2 concurrent, no cookie/auth/redirect, connection failure, withdrawal during response |

07-genre-input-regression.patch adds the integration evidence unit: unknown genre
zero cannot close; a measured positive subset can alert; complete known zero can
close only with complete current durable health.

The fixture lifecycle reset uses in_progress, not an invented active value,
verified from published migration
migrations/202611039998_s020_book_lifecycle_state.sql at 85f47ab9dbfa1ec15562e6e0602c61d08a9582fd.

The clones use LIKE public INCLUDING ALL in new test-owned schemas and app
search_path. They inherit installed columns, checks, indexes and identities but
are not proof of all B/C business triggers or foreign keys. Both these owner
tests and final installed-schema/real-server acceptance are required.

### Deterministic RR regression, not a timing guess

The test's view acquires a signal advisory key after the report snapshot exists,
then waits with pg_try_advisory_xact_lock and a 2ms pause on a gate held by the
writer. This keeps the production 75ms lock timeout unchanged. While the report
is paused, the writer atomically changes views 1→9 and extends the old incomplete
health interval to complete coverage, then releases the gate.

The combined port must return old views1 with incomplete old health. A later read
returns views9 with complete health. Reading old counts plus newly completed
health would fail the test and could otherwise manufacture a low-exposure alert.
No production-only test hook or permissive health bypass is added.

## Concrete restoration and execution sequence

1. Recover the known integrated local base and dirty worktree. Checkpoint that
   actual state first. Do not reapply patches01/02 to an already-modified worktree.
2. Restore the eight contents files only if they are genuinely missing. Apply the
   pending03/04 and supplemental05/06/07 in order after inspecting their contexts.
   The old absolute worktree prefix is the only path relocation assumed.
3. Copy the four prepared test files to the mapped paths. Complete existing 13-rule
   fixtures, additive686 migration fixtures, app module declarations, append-only
   RIL route408 / 409 pairs / 411 bindings, and shared H worker/render integration
   from their real source. Do not invent missing source or a routing fingerprint.
4. In the existing disposable-PG helper, with its explicit MIGRATOR_URL and
   TEST_DATABASE_URL, run the real current migrator and repeat it, then:
   cargo test -p tor-recommendations --test flow_genres_pg -- --ignored --test-threads=1
   cargo test -p tor-search --test public_pages_pg -- --ignored --test-threads=1
   cargo test -p tor-search --lib public_pages
   cargo test -p tor-web --lib flow_signal_worker
   cargo test -p tor-recommendations --test exposure_pg -- --ignored --test-threads=1
   cargo test -p tor-recommendations --test flow_signals_pg -- --ignored --test-threads=1
5. Run the installed-schema HTTP and real browser cases below. Only then run fmt,
   affected lint, repository/architecture/RIL guards, and publish acceptance
   against the actual integrated SHA. No command above has run for this package.

## Required HTTP/browser recipe on the actual server

Use the already accepted disposable PG and real tor-web process at a loopback
TOR_PUBLIC_ORIGIN; enabled public-page worker, real shared observation issuer,
and normal RIL composition. No production credentials or external deployment.

- Anonymous GET /admin/audience/pages is401; authenticated non-admin403; current
  administrator200; deactivate the role and repeat to get403. All success/error
  responses must retain no-store, noindex and the existing private CSP.
- Load the native report with JavaScript disabled at390 and1440. No horizontal
  overflow; four sample cards or explicit NoTarget; current canonical links only.
  A source GET must not change lease epoch, checked_at or stored baseline.
- After a real successful sample, induce an HTTP failure in the controlled test
  server without changing B/C eligibility. The source card and PublicPageLoss
  must show Lost/Active. Restore200, refresh the actual worker and signals, then
  require the exact current-sample sentence in both current state and history.
- Reproduce A Lost → lawful hold with no replacement. Current source must show
  NoTarget, no old A link. A fully known remaining sample may close only with that
  exact sentence; it must not claim A was restored.
- Add a new eligible B, change the publication revision, or switch configured
  test origin. Until a new200, evidence must be Unknown; no zero or recovered event.
  Simulate network loss separately: no invented HTTP500 and no recovery.
- Inspect all13 rules. A low genre sorted beyond detail100 must appear in the
  first20 explanatory low-label links; duplicate labels must not inflate totals.
  Selecting that exact genre must filter the real source report.
- With a fresh guest request context and explicit TOR-AvailabilityBot/1.0, compare
  the actual database's anonymous opportunity counters/proof count, HTML aggregate,
  and canonical reader-completion count before/after HEAD on the four targets.
  Require no increase attributable to HEAD. This verifies production middleware
  and routing; the small Axum service fixture alone does not prove it.
- Include future/stale-source and failed owner-read paths: no fabricated Healthy
  state or zero. Check the signal history preserves an existing episode through
  Unknown rather than emitting a false recovery.

Keep requests and fixtures bounded. Save actual logs/screenshots only after a real
run and associate them with the final integrated SHA. This supplement makes no
claim that any of these acceptance steps has passed.

