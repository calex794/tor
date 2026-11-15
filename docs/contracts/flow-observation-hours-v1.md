# FLOW-02/04: anonymous observation-hour counts

Status: implementation pending exact-head PostgreSQL/HTTP/browser CI acceptance.
Owner: F traffic behavior; I owns the additive migration and web composition.
This augments, rather than replaces, the existing FLOW roadmap. It does not close
FLOW-02 (visits/coverage), FLOW-04 (complete dashboard) or FLOW-06 (global quality).

## Product path

Site administrator -> Audience -> observed public HTML counts -> period
1/7/30/90 days -> UTC, Europe/Moscow, Asia/Yekaterinburg or Europe/Amsterdam.
Current and preceding periods contain complete UTC hours. Timezone changes the
labels and weekday/hour grouping, not the query's UTC boundaries. DST repeated
hours retain separate UTC keys and offsets. Detailed HTML shows at most the last
168 hours; all period sums use the entire interval.

A count is a successful eligible public HTML response observed by RIL, not a
human, session, confirmed reading, first touch or a purchase. Repeated page
opens remain repeated counts. Existing route/status/known-bot exclusions stay.
No consent UI, account key, cookie, IP, raw URL, search string, chapter/book ID,
client clock or individual navigation is introduced. REC/FLOW runtime
retirement from #806 remains active and is not bypassed.

## Data and execution

Capture server wall-clock UTC hour before enqueue, independently from the legacy
RIL batch-persistence hour. The existing bounded recorder/queue and fenced UUID
batch transaction carry a separate finite 4-surface x 4-source observation-hour
projection. A delta holds at most 48 different hours. Invalid clock, capacity or
counter overflow records an explicit discard; other valid cells survive.
Source categories are untrusted request-referrer hints, not verified SEO.

`ril_flush_flow_observed` is staging attached to the existing batch, not a new
queue. `ril_flow_observed_hourly` keeps anonymous counters for 180 days.
`ril_flow_observed_receipts` keeps batch projection dedupe receipts for 210 days,
longer than the existing 90-day RIL tombstones. Receipt and counter commit in the
same transaction after the existing lease/fence check. A mixed-version old
materializer cannot mark a new observed batch done without its receipt. The
trigger rejects that transaction so an updated worker can claim the batch.
Old batches without this projection remain compatible. No backfill interprets
legacy persistence time as observation time.

Receipt discard totals include in-memory delta discards and staged values older
than 180 days or more than one UTC hour in the future at materialization. They
are displayed by materialization time over the current complete-hour interval,
not attributed to an invented visit hour. They do not include losses before
queue admission, process death, unavailable peers or unobserved requests.
Cleanup is bounded to 10,000 rows per table per existing maintenance cycle.

## Read and access contract

The existing administrator GET accepts optional `time_zone` from the four-entry
allowlist. Fresh session/administrator checks precede query rejection. No new
route, API, mutable browser action or extra database pool is added. Existing
private/no-store/noindex/CSP and 950ms whole-response deadline remain.

The read-only repeatable-read query has a 750ms statement timeout, retrieves at
most 4,320 complete hour slots for two 90-day periods, and sums all eligible
surface/source rows. Missing cells stay unknown; measured zero stays zero.
Recorded-hour presence is not collection completeness. Old/current retired
individual reports remain visibly historical. Canonical reading facts and the
legacy technical report are unchanged. A failed new source yields HTTP503 with
a clear unavailable section, not zero traffic; other safe sections may remain.

## Acceptance

- Actual domain/recorder: bounds, invalid clocks, out-of-order original hours,
  hour rollover, missing/zero, checked sums, DST repeated slots.
- Existing `ril_rollups_pg` target: isolated migrated tables; retries, stale
  fences, multiple writers, replay after legacy tombstone deletion, cleanup,
  invalid codes, full 90-day dataset, actual IANA DST query, lock/recovery and
  installed-schema old-writer rejection.
- Existing `audience_hours_http_pg` target: real session/role, all periods/zones,
  fresh revocation, malformed query guards, locked observation table, recovery,
  bounded response and absence of identity/content.
- Existing hourly Chromium journey: real public navigation -> RIL -> PostgreSQL,
  negative API/404/bot traffic, independent observed-hour SQL expectations,
  period/zone form, unknown/zero, no document overflow and 390/1440 screenshots.
- Exact-head repository rules, formatting, clippy and full integration; then
  post-merge main checks. No production deployment or migration execution.
