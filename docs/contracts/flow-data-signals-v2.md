# FLOW aggregate signals: implementation defaults v2

This is a new implementation on the published #826 base. It is not a byte-exact
recovery of unavailable local commits. These numerical defaults are implementation
policy, not a claim that the product owner approved specific thresholds.

## Rules and sources

| Rule | Source and unit | Default active condition |
| --- | --- | --- |
| html_quality | H durable registered-worker health, complete UTC hours | Persisted loss/error or known partial coverage |
| html_change | H observed HTML requests | At least 30% fewer; previous count at least 100 |
| reading_starts | C first confirmed chapter starts | At least 30% fewer; previous count at least 50 |
| reading_completions | C first qualified retained completion per account/book | At least 30% fewer; previous count at least 50 |
| google_quality | Google import coverage and limits | Import error, incomplete known coverage or limited page detail |
| google_clicks | Google property-total clicks | At least 30% fewer; previous count at least 100 |
| yandex_quality | Yandex import coverage and limits | Import error, incomplete known coverage or limited page detail |
| yandex_clicks | Yandex property-total clicks | At least 30% fewer; previous count at least 100 |
| exposure_quality | F durable health and registered proof/render path | Persisted loss/error or known partial coverage |
| exposure_change | F saved browser-observed views | At least 30% fewer; previous views at least 100 |
| underexposed_books | F currently eligible books with adequate observed sample | At least one qualified low-observed book |
| underexposed_genres | F currently eligible genres with adequate observed sample | At least one qualified low-observed genre |
| public_page_loss | Latest bounded current public-page sample | Previously verified page has a confirmed unexpected loss |

Periods are 1, 7, 30 and 90 days, with 7 the UI default. Comparative windows have
equal duration and closed source-owned boundaries. Public-page sampling has one
latest scope (days=0), not four duplicate alerts. H/F health proves only coverage
of registered workers, never completeness of every installation or human audience.

Exposure classification requires at least 20 rendered cards, rendered/registered
proofs at least 80%, and fewer than 5 confirmed views for the low category.
Exposure changes require these sample/acknowledgement minima in both periods and
complete, loss-free durable health. The ratio describes the registered proof path;
it does not establish full audience coverage. Expiry and replay are not ingestion
losses. A positive qualified subset may open a low-exposure alert; zero cannot
recover it while any applicable entity remains unknown.

Missing data, unknown coverage, stale checks and canonical None are not zero.
Comparisons are suppressed for incomplete or failed measurement. Provider page
limits affect page-detail quality independently of complete property totals.
Source/configuration digests fence separate histories. No reader identifiers,
credentials, messages, automatic repairs or production activation are included.

## Persistence and timing

The worker claims a 15-second database lease before collecting a complete round.
The round epoch fences old writers. A round commits states and events atomically.
The default poll is 30 seconds; four periods rotate, with each revisited within
120 seconds. A state older than 300 seconds is displayed as unknown. Owner reads
and writes have whole-operation deadlines; round orchestration is bounded.

An active state is always visible. Opening events have a 24-hour cooldown.
Repeated observations do not create new events. Unknown, disabled, a changed
active reason, or a different source scope do not prove recovery. Only a healthy
observation for the same scope closes its episode. An opening suppressed by
cooldown has no recovery event. Event retention is 180 days in batches of at most
500; unresolved episode cursors are retained.

F health retains hourly evidence and closed epochs for at least 181 days.
Unresolved epochs are never removed merely because a lease expired or aged.
Counts retain the existing UTC anchor floor(now minus 900 seconds); health must
also confirm the callback settlement tail through each hour end plus 902 seconds.
Until this is demonstrated the window remains unknown.

## Acceptance status

Source checkpoints and draft tests are not acceptance. Actual Rust compilation,
fresh PostgreSQL and replay, concurrency/restart/failure tests, HTTP and browser
composition, and final integration CI are required on the published final SHA.
