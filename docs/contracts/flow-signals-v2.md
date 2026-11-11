# FLOW: aggregate signals and bounded source checks

This contract describes the repository implementation. A deployed worker and real
Google/Yandex credentials still require separate operational acceptance.

## Sources and meaning

The signal journal has thirteen fixed rules, with no reader identities, URLs,
personal history or behavioral REC input. HTML and canonical reading compare
adjacent equal windows of completed UTC hours for 1, 7, 30 or 90 days. Card windows
end at the UTC hour before now minus the 900-second proof lifetime. The owner
returns counts and collection health in one repeatable-read snapshot.

Canonical reading counts first verified starts per account/chapter and the first
retained verified chapter completion per account/book in the current public
catalogue. The latter is not a whole-book completion. HTML requests are not
people. Card observations are not people, attention or billable advertising.

The two search providers use their native report dates and independent property
totals. A bounded URL inventory does not invalidate an independently complete
click total; it does raise the provider quality signal. No live-provider success
is inferred from local wire tests or from an unconfigured source.

A card view requires at least 60 percent visibility continuously for one second
in the foreground. A low-exposure book or genre requires at least 20 rendered
cards, at least 80 percent observer acknowledgement, and fewer than five views.
The genre totals classify every nonempty genre before the bounded detail list;
overlapping labels do not multiply a book within a genre. Unknown entities
prevent a healthy zero from resolving the low-exposure signal.

## Evaluation and episodes

A drop requires at least 30 percent against a sufficiently large previous value:
100 for HTML requests, provider clicks and card views; 50 for canonical reading.
There is no inferred growth alert. These numerical policies are implementation
defaults, not estimates of people or causal attribution.

Missing, failed, stale, future-dated, disabled or incompletely observed sources do
not fabricate zeroes or recovery. A verified loss can remain active even if other
objects in the current sample are unknown. A change comparison requires complete
evidence for both windows. A confirmed idle HTML period may contain zero only
when its complete durable health evidence has no registered loss or failure.

Episodes are scoped by a SHA-256 digest of explicit source configuration.
Changing configuration does not close an old episode. Unknown or disabled
evidence preserves an unresolved episode. An opening message has a 24-hour
cooldown; suppression never hides the current active state or emits an orphan
recovery. GET renders results older than five minutes as unknown. Historical
events retain their original evidence and actual owner window labels.

Each round claims a 15-second database lease before owner reads. A successful
commit records all twelve period rules, or the single independent page rule,
atomically. Epoch fencing rejects stale writers and exact payload replay is
idempotent. The aggregate round has a five-second outer budget, at most two
parallel owner reads and 950 ms per owner operation. Rounds rotate periods every
30 seconds; public-page rounds run every 60 seconds. No network work holds an
owner database transaction. Event pruning removes at most 500 old rows per pass.
Events retain 180 days; unresolved states are not removed by age.

## Public-page sample

Four fixed slots cover home, catalogue, one eligible public book and one eligible
free chapter at the configured origin. Each round issues at most two concurrent
HEAD requests, without cookies, redirects or ambient proxy configuration.
Connect and request budgets are 300 and 750 ms; the entire probe is bounded by
five seconds. Eligibility and revision are checked before and after the HTTP
request and again before saving its result.

Only a previously successful HTTP 200 for the same eligible target establishes
a baseline. A subsequent different actual HTTP response can establish loss.
First failure, network error, changed source or revision, missing result and a
result older than three minutes remain unknown. Lawful withdrawal is no target,
not recovery. No claim about the entire site's URL inventory is made.

Both current state and recovery history say that no confirmed losses were found
in the current checked sample. They explicitly do not claim restoration of
previous URLs that are no longer in that sample.

## HTTP and lifecycle

The administrative entry points are /admin/audience/signals and
/admin/audience/pages. Each GET independently checks the live account session
and administrator role before validating query input or reading an aggregate.
A single 950 ms deadline includes authorization and the report. All successes
and failures are private, no-store, noindex/nofollow, with script execution
disabled. The journal uses bounded keyset history pagination. GET does not
evaluate, probe, modify a source, write the journal or create reader facts.

The main HTTP server handles SIGTERM and interrupt through Axum graceful
shutdown. Collector terminal acknowledgement is sent only after the actual
listener has stopped and all in-flight HTTP middleware has drained. HTML and
card workers then attempt bounded final persistence and retirement. Abort,
panic, a dropped control, timeout, failed persistence and listener errors do not
provide that witness. Unresolved epochs and tails remain evidence of unknown
collection coverage; a later process does not silently erase them.

The report remains distinct from paid advertising S042/S043. Editorial campaign
counts are owned site promotion and are not billable impressions. These signals
do not send external messages, change recommendation models or deploy anything.
