# FLOW-05: descriptive catalogue ranking and retained chapter evidence

## Scope and authority

This additive C read model finishes the repository-side catalogue-interest
questions without collecting new reader behaviour or connecting a person to an
exposure. It reads existing canonical C facts and their exact outbox proof.
It creates no event, profile, identity, migration, consumer, or write path.
Web composition independently requires a live site administrator and protects
every response with private no-store/noindex headers.

A numeric zero is a count of retained matching C facts, never proof that nobody
read a book. Account/book erasure and deletion of historical facts are not
reconstructed. Existing C facts are not subject to the retired FLOW raw-event
retention period. Missing source relations, permissions, timeouts, or DB failure
are unavailable, not healthy empty data.

## Full-scope descriptive ranking

`PgAudience::canonical_rank_report(&CanonicalQuery)` applies the accepted current
public/basic/general catalogue scope and current genre/tag filters. It uses the
same exact retained completion qualification as `canonical_report`: first
qualified chapter completion per account/book across retained history. Later
chapters and repeated completion requests do not create another pair.

Books, genres and tags are grouped across the whole filtered population before
selecting 20 rows. Order is descending current-window pairs; ties use ascending
book ID or exact C-collated group label, with the real null group first. No
string sentinel represents missing taxonomy. Duplicate labels within one book
are deduplicated. Whole-scope totals precede overlapping group expansion and
cannot be obtained by summing displayed rows.

The two adjacent rolling UTC windows share one read-only repeatable-read
snapshot. Each row supplies current and previous counts and explicit
`small_current_sample` / `small_previous_sample` flags for fewer than 10 pairs.
This is the existing presentation threshold, not a statistical significance,
privacy, quality, or universal demand threshold. The rank is descriptive and does
not change recommendation or public catalogue ordering.

The existing ID/label keyset report remains unchanged. Rank queries reject a
nonempty cursor rather than silently ranking only a page or ignoring it.

## Bounded evidence for the same closed exposure window

`PgAudience::closed_book_evidence(days, until_epoch, book_ids)` accepts only
1/7/30/90 days and at most 50 raw positive book IDs, before deduplication.
Each requested ID appears once in ascending order. No account, chapter, event,
purchase, or version identity is returned.

The exclusive UTC-hour boundary is supplied by the composing caller. C validates
it against the database clock in the same snapshot: the latest completed UTC
hour or the previous hour only. This permits a settled upstream window and one
hour-boundary crossing during the bounded request. Future, partial-hour,
arbitrary old, and invalid-period requests are rejected. C has no dependency on
the exposure feature or its proof lifetime.

For each requested currently eligible public book, both adjacent windows return:

- First verified starts per account/book/chapter in that window.
- **All** retained verified chapter completions in that window, including later
  chapters for an account/book whose first completion occurred years earlier.
  Repeated PUTs and rereading do not add canonical rows.

Every retained in-window C row must have the exact matching event type, schema,
aggregate, account/book/chapter/version fields, and free/purchased authority.
A missing or mismatched required outbox proof makes that book's affected period
`None` (unknown), including when some other rows in the same period qualify.
Absence/ineligibility of the requested book makes both periods unknown.
A successfully read eligible book with no retained rows has numeric zero.
Old or future unmatched rows outside both windows do not poison those windows.

F supplies its own settled `until_epoch` and durable registered-worker health.
Web may describe a book as having observed shows and no retained verified
chapter starts/completions only when the F sample and health requirements hold,
the C period is available, and both C counts are zero. A first-account/book-pair
zero alone is insufficient: an existing reader can complete a later chapter.
This is a same-window aggregate diagnostic, not exposure-to-reading attribution,
conversion, whole-book completion, or absence of actual reading. No global count
may be inferred by filtering one paginated exposure page.

## Bounds and acceptance

Both new C methods use read-only repeatable-read transactions, 750ms statement
and 75ms lock limits. Web retains a whole-request deadline including live
authorization and owner reads. PostgreSQL failures remain explicit unavailable
responses; they do not become zero or a missing-facts classification.

Acceptance must exercise a non-owner SELECT-only role, full-population top20
beyond the ID page, deterministic ties, overlapping/null taxonomy, filters and
small samples, old-reader/new-chapter evidence, exact half-open boundaries,
missing/mismatched proofs versus legitimate zero, unavailable/hidden books,
invalid IDs/windows, locked-source timeout and recovery. Existing canonical
keysets and qualification remain regression gates. HTTP/browser composition
has its own administrator, negative access, wording and no-JavaScript checks.

This contract specifies the implementation and its acceptance criteria. Source
publication alone is not a test result or production deployment.
