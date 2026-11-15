# FLOW-09 source-owner evidence

This contract describes the new source ports. Acceptance belongs to the final
integrated commit and its PostgreSQL, HTTP and browser results; a saved source
checkpoint does not establish that acceptance.

## Current public-page sample

The search owner probes exactly four slots: home, catalogue, the first currently
anonymous-readable basic/public book, and the first currently published, released,
free chapter. Missing eligible book/chapter slots are not replaced by private or
paid targets. This is a bounded current sample, not a site-wide URL inventory,
traffic counter, indexing report or independent external uptime service.

A probe is at most once per minute under a database epoch/lease. It performs HEAD
with no cookie or authorization, no proxy and no redirects. Only the configured
HTTPS origin is accepted; plain HTTP is permitted for loopback development.
Connect timeout is 300ms, request timeout 750ms, at most two awaited requests run
in parallel, and the whole round is bounded by five seconds. No database
connection or transaction spans a network request.

The current anonymous publication policy and book/chapter/publication revisions
are checked before and after HEAD and again under short row locks before save.
A stale epoch cannot write. Results occupy one state row plus four slot rows.
The public report uses a read-only repeatable-read transaction and rechecks the
current eligible targets before returning links.

A 200 response establishes a baseline for that exact origin and publication.
A later non-200 HTTP response is a confirmed loss only for the same still-eligible
target with a usable recent baseline. Network errors, first failures, changed
origin or revision, checks older than 180 seconds and future timestamps are
unknown. A newly selected book must first return 200. Lawful withdrawal is not
an outage. A clean remaining sample closes the current sample condition without
claiming the old URL has recovered:

> В текущей проверяемой выборке нет подтверждённых потерь. Восстановление ранее проверенных URL не утверждается.

These source probes do not create audience, exposure or reading facts. The web
integration must continue excluding HEAD from those collectors and test this
against the real application, not only the HTTP fixture.

## Closed C reading counts

PgAudience::closed_report accepts 1, 7, 30 or 90 days and returns one snapshot
with an exclusive completed UTC-hour boundary. Each period spans days * 24 hours;
the current and previous windows are adjacent and half-open.

Starts count retained verified first chapter starts, including authorized private
reading, as in the existing C audience aggregate. Completed book pairs count the
first retained verified chapter completion per account/book within the current
public catalogue, as in the existing canonical reading report. Later chapters do
not increase this pair count. Both require matching C outbox evidence, including
free/purchased authority and purchase ID when present. An invalid older row cannot
preempt a valid first completion. A valid retained first completion before both
windows prevents a later chapter from being counted as a new pair.

The two metrics have different units and catalogue scope and are not a conversion
funnel, unique people, complete-book reading or a visitor count. No account,
purchase, event or book identity leaves this numerical port. A successful zero is
zero retained matching C facts; failure is unavailability, not zero.

## Complete genre totals and one exposure/health snapshot

The exposure report calculates complete counts of distinct nonempty current
genres before the detail LIMIT 100. It returns total genres, low observed genres,
unknown genres and at most twenty low labels for explanation. Duplicate labels
within a book are counted once. Overlapping genres are not added to the global
book total. Pagination and the row-class filter cannot change whole-scope totals.

Low observed exposure requires at least twenty rendering acknowledgements,
at least 80 percent acknowledgements among registered opportunities, and fewer
than five view acknowledgements. Other insufficient samples remain unknown.
These are anonymous observations, not people or billable impressions.

PgExposure::report_closed_with_health reads the counts and durable health in one
read-only repeatable-read transaction. The shared boundary is the UTC hour floor
of database time minus 900 seconds. The health owner additionally requires the
hour's callback grace of 902 seconds to be confirmed; the newest hour can remain
unknown briefly. Session daylight-saving rules do not change window lengths.

The existing 181-day hourly retention keeps the oldest bucket of the 90+90 day
closed report. At the 900-second grace boundary this leaves at least 22 hours and
45 minutes beyond the earliest selected bucket. The actual prune port is covered
by the regression, rather than by a duplicated test-only delete statement.

## Regression targets

- tor-search/public_pages_pg: first failure, network, stale/future timestamps,
  origin/revision/publication changes, read-only access, lease fencing, redirects,
  HEAD-only credentials policy, slow real response and one-connection SQL progress.
- tor-reading/audience_closed_pg: exact C evidence, paid mismatch, missing events,
  old first/later chapter, current catalogue withdrawal, 90+90 windows, read-only
  access and bounded lock failure.
- tor-recommendations/flow_genres_pg: 103 groups beyond detail LIMIT, duplicate
  labels, page/class independence, concurrent counts/checkpoint commits, retention
  at the oldest closed bucket and UTC windows under DST/fractional-offset sessions.

The test fixtures clone migrated owner tables in disposable schemas. They do not
claim to re-exercise every production foreign-key trigger or the final HTTP
authorization path. Existing writer, migration and integrated browser gates remain
required.
