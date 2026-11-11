# FLOW-05: first retained verified chapter completion per account/book

## Administrator journey

A live site administrator opens Audience -> Confirmed reading by book
(`/admin/audience/reading`). Select 1/7/30/90 days, books/genres/tags and optional
current genre/tag filters. Missing labels have their own genuine null group.
A reader, moderator or revoked administrator cannot use this route or obtain its
numbers. Each response, including errors, is private, no-store and noindex.
The whole request shares a 950ms deadline, including live authorization; the
read-only PostgreSQL transaction has a 750ms statement timeout.

## Evidence and meaning

Only existing C `reader_chapter_completions` plus their exact matching committed
`ChapterCompletedV1` outbox events qualify. User, book, chapter, version, aggregate
and free/purchased authority must match; purchase evidence must also match when
present. An HTTP response, delivery grant, arbitrary event or unmatched row is not
a completion. The earliest retained qualified fact across all history for each
account/book is compared against two adjacent half-open rolling UTC windows from
one database snapshot. Later chapters, retries and rereading do not create a new
pair. Invalid early evidence cannot suppress a later valid fact. If earlier facts
have been removed, this remains a report of retained evidence, not all-time truth.

This is not completion of a whole book, number of humans/visitors, exposure,
source attribution, conversion or retention. A real zero means zero matching
retained C facts, not that a book has no readers or was never displayed. Source
failure returns private 503 and nonnumeric unavailability, not a successful zero.
No individual history, account IDs, source event IDs, purchase IDs or paid text
are returned or newly persisted. No optional tracking collector is reactivated.

Current library/follow links are counted separately from existing C tables.
Removal decreases current state; a later re-add is one surviving link, not
restoration of its history. These numbers are not all actions in the period.

## Current catalogue scope and groups

Use the existing F-approved C port `reader_return_public_books`; do not duplicate
F's visibility decision or add a new cross-feature dependency. It contains only
currently public/basic/general eligible books. A later hold/deletion/age change
removes book metadata and its numbers from this report without deleting C facts.
Taxonomy is current, not reconstructed historical taxonomy. Two specified filters
both apply. Duplicate labels within one book are deduplicated. Overlapping groups
can each include a book; global totals are computed before label expansion and
must not be obtained by summing the displayed groups.

At most 20 rows plus one pagination probe. Book ID and exact C-collated group label
keysets are validated; no offset scan, automatic popularity ranking or invented
"not shown" classification. Whole-scope totals are independent of the displayed
page. Descriptive current-minus-previous difference is shown only with at least
10 account/book pairs in each window; this threshold is a presentation policy,
not statistical significance or proof of anonymization.

## Acceptance and boundaries

Existing required `audience_pg`, `audience_http_pg` and audience Chromium targets
execute new paths; none is an optional substitute for exact-head CI. Tests use
real migrated tables and SELECT-only roles, adjacent boundaries, evidence mismatch,
old-first/new-later, future facts, duplicate/missing taxonomy, filters, >20 rows,
100000 completions and outbox facts plus 100000 links per product table, per-source
locks and recovery. HTTP/browser use the real C writer and role/session revocation,
CSRF-protected product choices, current visibility, 390/1440 screenshots and
escaped metadata. Negative route-governance tests reject removing authorization.

No migration, new event producer, queue, journal, tracking consent or Cargo
dependency. Production deployment and persistent databases are out of scope.
This closes one FLOW-05 slice, not the entire line or all catalogue-interest goals.

## Additive catalogue-interest reports

The opt-in top-20 descriptive ranking and the bounded same-window retained
chapter-evidence port are specified in [FLOW catalogue interest](flow-catalog-interest-v1.md).
They do not change this report's keyset ordering or first account/book meaning.
