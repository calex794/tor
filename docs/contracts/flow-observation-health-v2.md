# Registered observation worker health v2

This new implementation extends the published anonymous card-observation source.
Migration 202611294684 adds aggregate worker epochs and hourly health flags.
An epoch and its random registration-attempt key describe a worker, never a
reader, account, browser, IP address or cookie.

## What the evidence proves

A healthy interval means the registered workers acknowledged their observation
pipeline over that interval, with no known queue loss or source error. It is not
proof that every installation, HTML response, eligible card, person or browser
reported an observation. The rendered/registered ratio describes that registered
proof path only. Expired unseen proofs and replay callbacks are not ingestion
losses. Current-process queue counters remain separately labelled diagnostics.

Before a worker becomes ready, rejected issuance increments its loss counter.
Startup registration persists the known startup gap and any observed losses
before taking a new baseline. A DB clock sample minus process age measured after
the sample response conservatively bounds the startup gap. Host wall clocks are
not used for cohort attribution.

A startup transaction is idempotent under a fresh random per-attempt key.
If its commit succeeds but the response times out, retry discovers that same
epoch. Re-registration retires the caller's predecessor, preserves its unconfirmed
gap, and creates a successor in one transaction. Failure rolls all three back.
An expired lease alone never retires a worker.

## Issuance, queue and writes

Readiness has a 25-second monotonic deadline backed by a 30-second DB lease.
There is no timer-task assumption: a parked process stops issuing when a request
checks that deadline. After receiving a queued batch, the consumer checks
readiness again because a select guard may have been evaluated before a pause.

Issuance reserves queue capacity, increments its release-ordered issued counter,
then sends the item. A checkpoint samples its acquire-ordered target after
receiving the DB cut. A producer paused before sending remains included in that
target. The separately counted settled registrations must reach it before the
cut is confirmed. Persisted rows plus generic failures are not a substitute:
callback errors must not pretend to settle a queued card.

Registration and callback mutations acquire a shared live-epoch SQL fence inside
their bounded transaction and check it again before commit. An expired worker
returns unavailable (HTTP503); ordinary proof TTL expiry remains HTTP410.
Unavailable callbacks disable readiness and increment known source failures.
The write path retains the single DB instant for issue time, exact 900-second
expiry and UTC-hour cohort.

A pending queue target may renew a still-live lease without advancing confirmed
time. A lease that already expired cannot be renewed. An acknowledgement older
than 120 seconds requires a new epoch and a durable unknown gap.

## Closed windows and failure attribution

The existing count boundary remains UTC floor(now minus 900 seconds). Each
hour's health requires continuous acknowledged coverage through hour end plus
902 seconds, including the proof TTL and bounded callback-settlement margin.
Immediately around that boundary a latest hour can remain unknown.

A read requires both the union of acknowledged intervals and confirmation by
every registered epoch overlapping the extended interval. A non-retired epoch
has an open-ended tail even after its lease expires. A healthy second worker
cannot mask that unresolved tail. Retired intervals end at the witnessed end,
never at an old lease deadline.

New losses/errors flag affected issued cohorts from the previous confirmation
minus 902 seconds through a DB timestamp taken after the counter snapshot.
The upper floor-millisecond bucket is inclusive. Thus a failure after the cut,
including one exactly at an hour boundary, cannot be consumed into only the
earlier hour. Startup and handoff gaps also create explicit uncertain flags.

Comparative counts and health are read in one owner repeatable-read snapshot,
with the same persisted UTC boundary. Missing schema, failed queries, missing
coverage and unknown windows never produce a measured zero or recovery.

## Bounds and lifecycle

Repository operations have a 950ms deadline including pool acquisition; health
SQL has 750ms statement and 75ms lock limits. Observation writes retain 500ms
statement/50ms lock limits within their 950ms production deadline. No connection
is held while waiting for queue settlement.

Health reads cover at most two 90-day windows and 4096 overlapping epochs.
Exceeding the epoch cap returns unavailable. Retention removes at most 1000 hourly
rows and 1000 proved-closed epochs per maintenance call after 181 days.
Unresolved epochs are not removed by age.

The integration layer stops accepting and drains real HTTP requests before
sending the managed worker's shutdown witness. The worker closes/drains its
queue, confirms the final cut and then retires its epoch. A timeout, forced
abort, panic or dropped handle does not certify this witness. Such an epoch
remains unresolved until an actual lifecycle witness is supplied; there is no
automatic repair or claim of installation-wide completeness.

## Acceptance

The source contains PostgreSQL regressions for overlapping workers, crash tails,
startup losses, idempotent registration, stalled settlement, expired resumption,
atomic failed handoff, inclusive error buckets, timezone and SQL fences.
Managed drain/abort and HTTP503/history cases exercise the production worker path.
These tests must actually pass on the final integrated SHA; their existence or
an earlier local commit's results are not acceptance.
