# FLOW-07 / F-08: external search snapshots v1

Source plan: https://app.notion.com/p/41132a84beec81c08561f999c74144fb

## User path and boundaries

Administrator opens `/admin/audience/seo`, chooses Google or Yandex and
1/7/30/90 provider-labelled dates. A native POST `/admin/audience/seo/refresh`
coalesces seven recent dates. GET performs no writes. Fresh account authority,
Origin/CSRF on POST, private/no-store/noindex/CSP and a common 950 ms deadline
include authorization. No visitor, account, cookie, IP, search query, credential,
full referrer or reader history is collected by this feature.

The report uses official search-provider aggregates, not site visits or canonical
reading. A click from a search property is not proof of a page response, session,
read, purchase or a particular reader's conversion. Attribution to reading is
not fabricated. The local live-public projection filters landing-page rows;
property totals remain the provider's independent totals, not a sum of visible
rows. Retiring a public book removes its title/URL from subsequent reports.

## Provider contracts

Official sources checked for the implemented wire shape:
- https://developers.google.com/webmaster-tools/v1/searchanalytics/query
- https://yandex.ru/dev/webmaster/doc/ru/reference/host-query-analytics
- https://yandex.ru/dev/webmaster/doc/ru/reference/host-search-queries-all-history

Google requests final web search data. Property totals use byProperty; URL rows
use dimensions=[page] and auto/byPage aggregation, pages of 25000, at most 50000
rows/date. Google dates are America/Los_Angeles-labelled dates. Returning fewer
than the cap is not proof that Google exposes every query or row.

Yandex uses all/history for independent totals and query-analytics/list with
URL, WEB_LOCATION and ALL devices. At most 500 rows/request and 50000/date.
Returned date labels are preserved; no invented UTC reinterpretation. The
recent URL analytics window is finite: initial page history requests only the
previous 11 eligible dates. Older unavailable history is unknown. Successful
future imports accumulate the retained history.

The current UTC-labelled date and two preceding dates are excluded from all
reports to avoid advertising provisional dates as final. A period is half-open
[from,until). Google startup queues up to 180 eligible dates, Yandex up to 11;
restart does not reset completed dates. Hourly refresh requeues seven recent
dates, no more often than 15 minutes, and coalesces running jobs. A provider's
own late adjustment is reflected by atomic replacement, not incremental addition.

Only HTTPS configured-origin canonical `/`, `/catalog`, `/books/{positive_id}`
and `/reader/{positive_id}` rows survive. Query strings, fragments, user info,
foreign origins, noncanonical IDs, private paths and ambiguous encodings are
omitted, never persisted. Strict caps reject negative, fractional counts,
non-finite positions, inconsistent duplicates and malformed JSON.

## Storage and concurrency

F/search owns `flow_seo_sources`, `flow_seo_daily`, `flow_seo_pages` and
`flow_seo_jobs`. One enabled exact property per provider. No new reader events,
collector consent scopes, shared tracking identity or business outbox producer.

Claim uses SKIP LOCKED plus a 180 second lease and monotonic epoch. Network I/O
holds no PostgreSQL connection. A whole collect is bounded to 100 seconds;
individual HTTP requests to 8 seconds, connect to 2 seconds and response to 8 MiB.
Only fixed official origins are used; redirects are disabled. Credential headers
are sensitive and raw provider errors are never logged or returned to users.

Finish checks enabled source and exact job/epoch/deadline, then replaces one
complete source/day in one transaction. Failures and stale writers cannot erase
or overwrite the previous valid snapshot. Error codes and exponential retry are
bounded (maximum one hour). Read transactions are repeatable-read/read-only,
750 ms SQL and 100 ms lock. Write SQL uses 3 seconds, 100 ms lock. Indexed live
numeric-ID joins precede LIMIT 51; keyset pages display 50 rows, no OFFSET scans.

A coverage gap is not zero traffic. A successful explicit numeric zero differs
from an absent total. Current/previous deltas require complete totals for both
periods. Page sums and property totals can differ legitimately. A weighted
position is shown only from known positive-impression measurements.

Retention deletes at most one source/day (up to the import cap) and 100 old jobs
per cycle beyond 200 dates. An expired old running job may be removed, a live
lease may not. This is bounded cleanup, not an immediate full history purge.

## Operator activation (not performed by the PR)

Defaults make no provider requests. Production deployment/migrations and token
setup remain separate, explicitly authorized operations. Enable only with:

```
TOR_FLOW_SEO_IMPORT=1
TOR_FLOW_SEO_ORIGIN=https://your-public-origin.example
TOR_FLOW_GOOGLE_PROPERTY=sc-domain:your-public-origin.example
TOR_FLOW_GOOGLE_TOKEN_FILE=/absolute/private/google-token
TOR_FLOW_YANDEX_PROPERTY=https:your-public-origin.example:443
TOR_FLOW_YANDEX_USER_ID=<positive user id>
TOR_FLOW_YANDEX_TOKEN_FILE=/absolute/private/yandex-token
```

Configure only providers actually owned and authorized. Google exact HTTPS URL
prefix properties also work. Token files must be regular, absolute, private to
the process owner (0600 on Unix), 8..4096 ASCII token bytes. Files are reread for
rotation; no secrets go in Git, SQL, browser forms or reports. Google access token
renewal must be supplied by the deployment's credential mechanism; this package
does not pretend an expiring access token is permanent OAuth authorization.

Worker is independent of user AUTH flag. Invalid configuration does not create
fake empty sources or prevent normal reading. Source errors keep last valid
measurements. No real Google/Yandex property has been connected or activated by
the implementation/test run.

## Acceptance

`cargo test -p tor-search --lib` covers official-shaped loopback wire requests,
parsing, origin restrictions, missing versus zero and provider failures.
`cargo test -p tor-search --test seo_pg -- --ignored --test-threads=1` covers
fenced replacement, concurrent claims, stale lease, failure preservation,
backfill/restart, provider separation, live hiding, retention, read-only role,
keyset pagination and 50000 landing rows. This scale check is one date, not an
unmeasured promise for every 180-day dense catalogue.

The existing mandatory `audience_http_pg` test includes the real admin route,
all periods/providers, GET authority, POST CSRF/Origin, oversized form,
coalescing, independent lock503/recovery and role revocation. The existing
`audience_pg_browser.cjs` invokes the SEO helper: real no-JS forms, 50+3 rows,
live book hiding, independent property totals and 390/1440 images. Tests use
local disposable tor_ci only. Provider wire fixtures are not live access proof.

Before acceptance: exact-head mandatory CI, inspection of CI screenshots,
ordinary PR merge and exact post-merge CI. No deployment or production mutation.
