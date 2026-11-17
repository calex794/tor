# G-05b — Cover storage inventory before any lossy-WebP backfill

**State:** read-only audit, never an automatic migration or converter.
The WebP q85 book-cover recipe from #1017 is already merged, but is default-off
(`TOR_COVER_LOSSY_WEBP_ENABLED` is not enabled here).

## Why we measure first

TOR's current book cover is **not a mutable filename**. It is an approved
`book_cover_slots` job/revision pointing at an immutable private `cover_variants`
key and SHA-256, and may also have a corresponding immutable public CDN copy
and purge ledger. Book-cover changes normally require fresh moderation.

Blindly overwriting the old WebP with lower-quality bytes would:
- violate existing checksums and immutable S3 keys;
- invalidate an approved moderation/publication identity;
- risk stale browser/CDN copies and broken reader images;
- hide the actual original-object retention state.

`cover_upload_objects.status='verified'` is **metadata**, not proof that
its S3 file can still be fetched; source cleanup may also have retired or
removed prior uploads. Re-encoding must never be inferred from this report.

## Run (read-only PostgreSQL)

With least-privileged `PGHOST`, `PGUSER`, `PGDATABASE`, `PGPASSWORD` / standard
libpq credential configuration set for the intended *read-only account*:

```sh
psql -X -v ON_ERROR_STOP=1 -f tools/cover_lossy_inventory.sql
```

The SQL explicitly begins a **REPEATABLE READ READ ONLY** transaction, sets a
15-second statement timeout and a 2-second lock timeout, prints only aggregate
counts and byte totals, then rolls back. It has no S3 or HTTP operations,
deletion calls, admin endpoint or scheduled invocation.

### Four result groups

1. **Current cover slots by recipe:** count of books, public-approved slots,
   withdrawn slots, pending replacements, verified/retired source records,
   complete main/card pairs, recorded bytes of both variants and originals.
   `legacy_metadata_candidates_for_manual_review` is only a conservative
   **metadata shortlist** for future operator review; it does *not* authorize
   conversion. It excludes inactive/withdrawn/unapproved books, missing
   variants, pending replacement review, retired sources and mismatched owners.
2. **All private variants by recipe:** ledger bytes, including history; count
   marked removed by the private-retention system. This is NOT physical S3 usage.
3. **Source uploads by status:** aggregate original-object metadata, including
   content that is not a current cover; presence in S3 is unverified.
4. **Public CDN publication ledger by state:** aggregate registered copies,
   including historical identities and pending purges. This is NOT CDN traffic
   or billable edge-cache storage.

No book IDs, owner IDs, source URLs, keys, hashes or book titles are emitted.

## Cost interpretation

To estimate storage savings, start with `recorded_current_main_bytes` and
`recorded_current_card_bytes` for legacy rows. **Do not** apply the
single-cover q85 ratio to every book as an established average. Instead, sample
50–100 diverse, legitimately available original covers **offline**, measure
actual v1/v2 bytes, text legibility and CPU overhead, then construct a
distribution, including variability and failure cases.

Separate billable categories:
- source/original S3 objects (unaffected by variant-only conversion);
- attached main/card variants in private storage;
- historical/staged variants that haven't been confirmed removed;
- approved immutable CDN origin copies;
- cache hit rate and actual data delivered to readers.

## Safe prerequisites for any future backfill PR

1. Operator selects a *bounded* sample of current, approved v1 slots. No
   production processing or automatic queue on this audit.
2. Reverify *actual* original S3 bytes and matching size/SHA-256; missing
   originals must be skipped, never reconstructed by multiple lossy passes.
3. Encode **new immutable** main/card files under distinct keys. Never rewrite
   a byte under an existing checksum or CDN key.
4. Establish a **separate approved promotion path** preserving book owner,
   current-slot CAS revision, moderation audit and any pending author cover
   replacement. No direct SQL flip of `book_cover_slots`.
5. Reserve, confirm, and withdraw public CDN copies via the existing
   publication/removal/purge ledgers, preserving reader access on failures.
6. Define rollback and cleanup before any batch; record metrics for rejection,
   CPU, memory, bytes, errors and CDN state.
7. Prove all steps with disposable PostgreSQL/MinIO/Chromium CI and manual
   review of representative cover quality before an operator-only rollout.

No migration, VPS update or S3 operation belongs to G-05b.
