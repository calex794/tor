# REC contextual metadata/manual model v1 (FLOW/REC P2b2)

## Scope and source

Version `metadata_manual_v1` is a new explicit model, not a rename of the retired
behavioural generations. It reuses `rec_generations`, `rec_candidate_edges` and
`rec_runtime_policy`. It has no reader history, new analytics, event producer,
queue, consent control, assignment, impression, click ticket or behavioural score.
Canonical reading, purchases and explicit P1 hide/restore choices keep their owners.

Reference: NEwBOO `backend/app/recommendations/services/hybrid_scoring_service.py`
at immutable `17125ef758a791e9630505c14d24f7fe65aed228` (repository HeisLuka/newboo).
Its separate manual-related channel and content-only weighting are reused, not its
behaviour/popularity pipeline. Existing TOR `sources::book_sources` supplies
180 per shared genre (max three), 120 per shared tag (max five), plus 10 for the
same language after nonempty overlap, capped at 1000. No author identity is
inferred from owner/display-name fields. Manual links are a distinct channel,
ranked before content. One book appears at most once. Existing per-tier content
partitions (20 each) and the 10 manual links remain bounded at materialization.

## Serving and live rights

Only a concrete authorized book page context may use the active model. The global
surface, no active model, hold, unavailable model storage or no remaining eligible
edge uses the ordinary live new-release catalogue. `personalized=false` describes
this non-behavioural ranking, not the absence of account-level access checks or
explicit user settings. Every returned item has no analytics ticket.

The feed checks live seed/candidate discovery, age and account state and P1 hides
before its final LIMIT. Manual links and content language/genre/tag overlap are
rechecked live. Removing a link or hiding/restricting a book takes effect without
rebuilding. Adding new links/books or changing a score needs an explicitly requested
new generation; old scores are snapshots, not a claim of continuous retraining.
A changed but still overlapping metadata set can retain its old score until rebuild.
No identity hash or percentage audience assignment is used: rollout is 0 or 100.

The optional contextual read is read-only, has a 400ms whole-operation timeout,
300ms statement timeout and 75ms lock timeout. Timeout/error/empty result falls
back to the pre-existing catalogue path, not an unfiltered stale model. These
limits do not promise an end-to-end SLA for every catalogue/database outage.

## Build, activation, hold, rollback and cancellation

Native administrator page: `GET/POST /me/recommendations/contextual`.
JSON control: `GET/POST /api/v1/admin/recommendations/contextual`.
Fresh administrator/session and Origin/CSRF checks remain mandatory. Both UI and
JSON call the same operation; no user IDs or rollout percentage are accepted.
Unknown fields are rejected. Whole HTTP operation including authorization is
bounded to 950ms. Reads/commands have 750ms statement timeouts and private
no-store/noindex/CSP responses. Failure renders no fabricated status or enabled
mutation controls. Existing retired control endpoints still return private 410.

Each command requires `expected_revision`, locks the single policy row and
increments the revision atomically. A stale/replayed revision is 409 without a
second mutation. This is CAS control, not an idempotency receipt that replays a
prior successful result. Refresh coalesces an existing building generation;
otherwise it creates exactly one explicitly labelled building generation.
Activate accepts only a completed validated new-model UUID. Hold affects serving
but does not delete models. Rollback selects only the preceding validated
new-model version. Cancel changes only a building new-model generation to rejected.
An abandoned/busy/erroring build can be explicitly cancelled; no partial build is
activated. The product flag remains independent: model activation does not turn
on `TOR_RECOMMENDATIONS_ENABLED`.

A separate process task, using the existing shared pool, advances only explicitly
requested new-model generations. It runs even with AUTH/reader-serving disabled;
it does not call the old generation builder, global behavioural sources, retired
consumers or collection callbacks. Each transaction locks one building generation
with SKIP LOCKED and processes one ascending seed. Cursor and edges commit together.
Another worker skips the same locked generation. Worker deadline is 2 seconds per
transaction, 250ms pause after progress and 5 seconds after idle/error. Restart
resumes from the committed cursor. The last empty page validates the generation;
its 32-byte manifest is a completed-build receipt derived from model/ID/cursor/edge
count, **not** a hash of all book metadata or proof of recommendation quality.

The operator list returns the latest 20 versions. This bounds one response, not
total persistent generation storage. Automated deletion/GC is not introduced.
The scale test builds one real page against 100000 metadata candidates while a
reader shares a three-connection pool. It is not a complete 100000-seed generation
benchmark or a guarantee of full-build duration. Dense full rebuild cost remains
dependent on catalogue size/overlap; cancellation and bounded reader isolation
are available without weakening timeouts.

## Installed SQL transition and immutable boundaries

Additive migration `202611198437_rec_contextual_model.sql` follows accepted 193.
All accepted checksums remain unchanged. Old rows are labelled `legacy_feedback_v1`,
old building generations are rejected, and old active/previous pointers are
cleared with hold/rollout=0 before the new guards are installed.

SQL rejects default legacy inserts, relabelling generations, changes to completed
generations, invalid model activation, identity-based rollout and old behavioural
edge sources. Edge writes lock the building parent against simultaneous validation.
The old assignment foreign key changes SET NULL to RESTRICT: deleting an
unreferenced generation must not invoke a forbidden retired UPDATE, while a
surviving historical assignment keeps its referenced generation intact.
The retirement write fence is never disabled. SQL cannot erase an old process's
memory; old collectors must still be stopped by the separate approved release.
No production database, external service or deploy is changed by this PR.

## Required acceptance

* Installed190 -> actual retirement193 -> contextual194, twice, with all earlier
  checksums and C/P1/anonymous projection preservation; old-model activation,
  relabelling and unfinished activation negative cases.
* AUTH=0 production binary on genuinely updated non-drained data: readiness,
  independent P1 expiry, busy retirement gate, drain recovery and late C command.
* Real metadata/manual builder -> activation -> live feed, P1 hide/restore,
  concurrent CAS, cancellation, parent/edge fencing, rollback only to new models,
  source locks/read-only roles and 100000-candidate bounded shared-pool page.
* HTTP session/admin/Origin/CSRF/CAS, product flag independence, source lock503 and
  recovery without restart, no newly collected subject/assignment/history rows.
* Real no-JavaScript admin forms and real process builder, two generations,
  activation/rollback/hold, existing B manual-link form, live hide/revoke and 390/1440
  screenshots with privacy and overflow checks. Existing REC/browser gates retained.

The saved contract is not evidence of complete acceptance. Exact-head CI, inspected
actual screenshots and exact post-merge checks remain mandatory. P2b2 does not
complete the remaining FLOW SEO, campaigns, exposure diagnostics or alerts.
