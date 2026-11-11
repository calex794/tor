# FLOW-05: explicit library and book-follow snapshots, v1

Status: implementation candidate; full FLOW-05 is not accepted by this slice.
Scenario: site administrator -> `/admin/audience` -> libraries and book follows
-> 1/7/30/90-day selection. Normal readers never receive this report.

## Sources and meaning

C owns `reader_library_items` and `reader_book_follows`: these are existing product
choices, not optional REC/FLOW analytics events. The report is SELECT-only and
creates no events, identifiers, subscriptions, visitor profiles or consent prompts.
The runtime retirement decision remains unchanged. No table or migration is added.

All counts are calculated in one read-only repeatable-read transaction. The same
UTC transaction anchor defines adjacent half-open intervals of exactly N*24 hours.
The viewer's hour-report timezone does not change these UTC rolling intervals.

- `totals`: rows that currently exist, including rows older than both intervals.
- `current`: rows that still exist and were created in [now-N days, now).
- `previous`: rows that still exist and were created in [now-2N days, now-N days).
- Accounts are COUNT(DISTINCT user_id) within each choice/cohort, never a sum of
  per-day unique counts. Library and follow account counts overlap and cannot be
  added to claim unique people or site visitors.

Both periods are queried from today's snapshot, not from two historical snapshots.
Removed choices are absent. A repeated command preserves one row and its original
creation date; deleting and re-adding creates a new row/date. Therefore the cohorts
are NOT all additions, historical audience size, net growth, retention or conversion.
Future-dated rows, if present in stored product state, count in the current total
but not in either creation cohort. Book moderation does not rewrite these anonymous
counts; no titles, book/account IDs, purchase data or chapter text leave SQL.

An empty successfully queried product table is measured zero. Source failure is
unavailable, never zero. A missing historical row is not reconstructed. Completed
chapters remain distinct from completed books; this slice makes no book-completion
claim and does not close the remaining FLOW-05 book/genre/tag breakdowns.

## Bounds, access and independence

No new route or permission is introduced. Live account/session/administrator checks,
no-store/noindex and the existing CSP remain mandatory. An expired role loses access
immediately. Existing Origin and CSRF checks on library/follow writes are unchanged.
The page has one 950ms deadline including authorization and all sources; SQL has a
750ms statement timeout. Failure of this source yields private503 while independent
successful aggregates remain visible. Releasing a lock restores the report without
a server restart. Each source table is scanned once; acceptance includes 100,000
rows per table under a SELECT-only, default-read-only application connection.

## Executable acceptance

- `cargo test -p tor-reading --test audience_pg -- --ignored --test-threads=1`:
  migrated table copies, real zero, exact whole-cohort account distinctness,
  1/7/30/90 windows, half-open boundaries, non-UTC PG session, future dates,
  deletion/re-addition, SELECT-only privilege, lock/unlock recovery and 100k rows.
- `cargo test -p tor-web --test audience_http_pg -- --ignored --test-threads=1`:
  real guarded save/follow/replay/remove commands, bad CSRF rejection, live admin
  checks, source-specific 503/recovery, no identity/content disclosure.
- `tools/audience_pg_browser.cjs` calls `audience_intent_pg_helpers.cjs` in the
  existing PostgreSQL/Chromium journey: actual product POSTs, repeated commands,
  removals, SQL-backed report, 390/1440 screenshots and no horizontal overflow.
- Unit rendering distinguishes unavailable from zero and escapes timestamps.
- `tests/architecture/test_flow_explicit_choices.py` protects read-only boundaries,
  authorization/deadline wiring and mandatory CI execution of the existing targets.

Acceptance requires exact-head integration and Repository rules, inspection of real
browser images, ordinary PR merge and post-merge verification. Tests prepared in
source are not evidence of passing execution. No deployment or production SQL.
