# FLOW-05 — anonymous card observations and low observed exposure

Owner: F (`tor-recommendations`); shared types: `tor-contracts::exposure`;
HTTP/composition: I (`tor-web::flow_exposure`). Editorial campaigns remain owned
by `tor-ads` and enter through typed ports, never shared ad billing tables.

## Meaning of each count

| Field | Evidence | What it cannot establish |
| --- | --- | --- |
| `opportunities` | A random proof was committed for a trusted server-rendered card. | Delivery of every HTML response, visibility, complete traffic coverage. |
| `rendered` | Browser callback acknowledges that card in its DOM; a valid view/click also implies this acknowledgement. | Visibility in the viewport or an independent person. |
| `views` | Browser reports at least 60% continuously in the viewport for at least 1,000 ms while `document.visibilityState === 'visible'`. | Unique people, fraud resistance, billable advertising, whole-book reading. |
| `clicks` | Browser reports a normal click on the trusted card link. | A view, successful destination load, purchase, or attributed reading. |
| `expired_unobserved` | A proof expired without a confirmed view. | That the card was unseen; the callback may be missing. |
| `replayed` | An already accepted event bit was received again. | Additional exposure or pipeline loss. |

These are counts of card observations, including repeated cards on one page or
different pages. They are never renamed visitors, accounts, unique readers, or
paid impressions. Client signals can be forged by someone holding a valid proof;
this collector is deliberately unsuitable for advertiser settlement.

Without a DOM acknowledgement, view/click values are **unknown**, including when
JavaScript is disabled, the API is blocked, or no registered proof exists. When
DOM acknowledgements exist, zero means zero accepted callbacks of that kind. It
does not prove no one saw or clicked the book.

## Low observed exposure

For each currently eligible public book and selected interval/source/surface:

1. Require at least 20 DOM acknowledgements.
2. Require DOM acknowledgements / registered opportunities to be at least 80%.
3. With both conditions satisfied, fewer than five confirmed views is **low
   observed exposure**. At least five is **observed threshold reached**.
4. Otherwise the class is **insufficient observations**.

The denominator is only persisted proofs, not all page traffic. Queue losses
cannot be inferred from that ratio. These operational thresholds identify books
whose placement should be inspected; they are not a demand, quality, statistical
significance, or causal advertising metric. Independent canonical reading C facts
must not be divided by these card counts and called conversion. No CTR is offered:
click and view callbacks can arrive independently and out of order.

## Trusted issuance and anonymity

Only a trusted Rust renderer calls `ObservationIssuer::issue(CardObservation)`.
Input has a positive book ID, a fixed surface, and optional typed editorial
placement `(campaign_id, revision, slot)`. It contains no reader identifier.
Author-provided chapter HTML is never parsed to obtain issuance metadata.

The issuer obtains 32 random bytes from the OS, returns a canonical 43-character
base64url token, and enqueues only its SHA-256 digest and trusted card metadata.
No new cookie, browser storage, user/account/subject ID, IP, session identifier,
referrer, search query, URL history, or reading history is stored. The random
per-card token is independent of all other cards and expires 15 minutes after
registration. The database stores its digest, not the raw token.

Successful enqueue does not mean successful registration. The bounded worker
must commit proof and opportunity in one transaction before a callback can be
accepted. If that fails, the card stays usable and measurement is unknown.

Before publishing an issuer, the runtime checks the independent observation
migration, three tables, immutable-proof trigger, and initialized state row.
`PgExposure::available()` is read-only and has a 300 ms total deadline including
pool acquisition. Missing schema, legacy fixtures, or unavailable readiness do not
receive proof attributes. No migration or cleanup is run by this check.

Instrumented surfaces are public home, catalogue, search, book-detail contextual
recommendations, free-reader contextual recommendations, and the three editorial
placements. Private libraries and paid-reader pages are excluded. HEAD requests
receive no issuer: Axum may execute a GET handler but discards its HTML body.
Admin editorial previews pass no issuer. Ordinary links and forms work without JS.

HTML responses served with an issuer are `no-store` and have no ETag before any
body transformation. The observer script is added only to bounded, complete UTF-8
HTML with a closing body tag. Larger than 2 MiB, unknown-length, and incomplete
HTML bodies are passed through unchanged with the same cache isolation; missing
browser signals remain unknown. Ordinary text containing the asset URL cannot
suppress script insertion. CSP directives are separated explicitly even if the
original policy has no trailing semicolon; existing unrelated restrictions remain.

## Browser and HTTP protocol

Trusted cards receive `data-flow-proof`; only their ordinary `data-flow-link`
anchor is observed. The production script is served at
`GET /assets/flow-observations.js`. It scans at most 128 SSR cards, sends DOM
acknowledgement, observes actual intersection geometry, and resets the continuous
timer when the card is below 60%, the document becomes hidden, the node is
disconnected, or the page leaves. Each event kind is attempted once per card.

`POST /api/v1/flow/observations` accepts at most 4 KiB of JSON:

```json
{
  "proof": "canonical_43_character_random_server_token",
  "kind": "view",
  "visible_percent": 60,
  "continuous_visible_ms": 1000,
  "foreground": true
}
```

Kinds are `rendered`, `view`, `click`. Qualification fields default to zero/false
for non-view events. Unknown fields are rejected; book, campaign, slot, revision,
actor, and timestamp never come from client input. Origin must be same-origin
when present and `X-Tor-Observation: 1` is mandatory; cross-site fetches are not
granted CORS. No CSRF or identity cookie is created by this endpoint.

| Status | Meaning |
| --- | --- |
| 204 | Accepted once or already accepted; no token/identity is echoed. |
| 400 / 422 | Invalid qualification/token or malformed JSON/unknown fields. |
| 403 | Origin denied, revoked/invalid existing session, current book rights/preferences denied, or editorial placement no longer valid. |
| 413 | Request exceeds the body limit. |
| 425 | Proof is not yet registered, or a view arrived before one server second elapsed. |
| 410 | A still-retained proof has expired. |
| 503 | Bounded database/guard operation unavailable. |

The client retries only 425/503, at most three times after 150, 400, and 1,200 ms.
Fetch uses keepalive and never prevents, awaits, or rewrites navigation. Network
failure stays missing evidence. Every API response is `no-store`.

Existing authentication, if present, is resolved afresh only to check current
book visibility, active account, age permission, and explicit Hide preference.
This identity is an ephemeral authorization input and is absent from all new
rows and counters. Anonymous callbacks can only observe basic/general books.
Editorial callbacks also invoke the editorial owner's live guard for the exact
issued campaign/revision/book/slot/schedule. No guard means no editorial receipt.

## Atomicity, limits, and retention

Migration `202611286493_flow_anonymous_observations.sql` creates independent
`flow_observation_state`, `flow_observation_proofs`, and
`flow_observation_hourly`. It neither writes to nor bypasses any retired
individual-collection table or trigger. Issued proof metadata is immutable;
accepted bits cannot move backwards. Aggregate constraints enforce
`views <= rendered <= opportunities` and `clicks <= rendered`.

One transaction locks a proof, revalidates current access, deduplicates its event
bit, and advances the matching hour/book/surface/campaign/slot aggregate. A click
does not set the view bit. Reordered callbacks and process restarts retain dedup.
Hourly buckets are issuance cohorts; callbacks can arrive later within the TTL.
Registration derives issuance time, exact 900-second expiry, and the UTC bucket
from one database clock instant, including when a transaction crosses an hour.

The queue is limited to 1,024 items, registration batches to 64, and live proof
storage to 100,000 rows. SQL mutations use 500 ms statement / 50 ms lock limits;
the worker cancels operations after two seconds. No connection is held while
waiting for a browser action. Every five seconds cleanup deletes at most 1,000
expired proofs and at most 1,000 aggregate rows older than 181 days. Expired proof
storage can outlive its acceptance TTL while this bounded cleanup catches up.

Current-process queue diagnostics count queued, persisted, dropped, and failed
registrations/cleanup. They reset on restart and do not prove historical
completeness. Durable source-health intervals for change alerts are a separate
FLOW-06/09 integration; replay and expiry are not mislabeled ingestion losses.

## Administrative report and integration ports

`GET /admin/audience/exposure` requires a current A administrator session. Auth
and all report/guard reads share a 950 ms HTTP deadline. Reports use a repeatable
read, read-only transaction with 750 ms statement / 75 ms lock limits. Responses
are `no-store`, `noindex, nofollow`; errors show unavailable, never invented zeros.

Native GET forms accept `days=1|7|30|90`, fixed `surface`,
`source=organic|editorial`, `campaign=<positive id>`,
`class=unknown|low|observed`, `after=<book id>`, and
`group=genre|tag&label=<exact label>`. An empty label selects books without that
label type. Exact filters and cursors survive pagination. Pages contain 50 books.

Overall totals precede pagination/class filtering. The current catalogue's
visibility gates and current metadata apply to books, groups, and totals.
Repeated genre/tag labels do not duplicate a book inside its group. Groups
overlap; their numbers must not be summed. The initial group list is bounded to
100 alphabetic groups; the exact-label form reaches any group.

A campaign-specific report resolves its immutable assigned book through
`EditorialObservationGuard::campaign_book` after administrator authorization.
`PgExposure::report_campaign_book` requires this trusted scope, even before the
first opportunity; an unmeasured assigned book is unknown. Missing integration is
unavailable and an unknown campaign is 404. Other catalogue books are excluded.

`PgExposure::report` shows calendar days including the partial current UTC day,
with a labelled previous interval. It does not certify comparability.
`PgExposure::report_closed` is the signal-owner port: two equal `days * 24h`
intervals ending at `date_trunc('hour', UTC now - 15 minutes)`. This waits a full
proof TTL after the latest hour closes. Signal comparability also requires its
separate durable health evidence; equal time alone is insufficient.

## Acceptance

- PostgreSQL: `cargo test -p tor-recommendations --test exposure_pg -- --ignored --test-threads=1`.
- HTTP: `cargo test -p tor-web --test exposure_http_pg -- --ignored --test-threads=1`.
- Production JS with deterministic visibility/clock: `node --test tools/exposure_client.test.cjs`.
- Real browser against a current migrated disposable `tor_ci` server:
  `APP_URL=... TOR_EXPOSURE_BASE=http://127.0.0.1:3109 node tools/exposure_pg_browser.cjs`.

Browser evidence covers actual geometry/continuous timing, no-JS unknowns,
native admin filters, 390/1440 layouts, replay/reorder, withheld callback during
navigation, and current withdrawal. Headless-shell keeps every tab visible;
hidden-tab transitions are tested deterministically against the production JS
and are not represented as physical background-tab browser evidence.
