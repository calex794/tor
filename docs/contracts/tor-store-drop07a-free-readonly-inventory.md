# TOR DROP-07a — read-only FREE SINGLE/PACK inventory

**Status: Draft, test-only implementation.** The existing paid/ciphertext G-09 audit does not prove integrity or completeness of new FREE namespaces. TOR begins with an empty catalogue: no historic migration or backfill is planned.

This first slice introduces **no migration, endpoint, timer, operator repair, S3 DELETE or automatic GC**.

## B — expected registry

PgFreeExpectedInventory::expected_page(bucket, after_key, limit) enumerates at most 128 immutable registered objects per call, ordered by exact physical key:

- S349 source SINGLE from free_chapter_storage_objects, with immutable SHA-256 and size.
- Complete registered PACK from free_storage_packs, with full-object SHA-256 and size.
- Its immutable sidecar manifest from free_storage_packs.manifest_key, with SHA-256 and size.

Only a validated private bucket may be queried. Malformed/traversing cursors fail closed. Keys are internal worker data, **never returned by public HTTP**. Concurrent writes can insert a key before the cursor: a successful traversal is *not* a consistent snapshot. A future fenced checkpoint or an independent repeat cycle is required.

Pending R4a jobs, in-flight PUTs, staged PACK-direct receipts, rollback SINGLE, pinned FREE V1 and separately protected backups are not necessarily active-pointer references. They remain protected in future GC classification. Absence from a registry page is **not evidence of an orphan**.

## G — actual-byte verification

S3FreeExpectedObjectVerifier::verify_exact uses a bounded private GET, at most 16 MiB + one byte, and compares actual length and SHA-256 of returned bytes. Verified, SizeMismatch and HashMismatch are returned for actual downloaded bytes. S3 outage, ambiguous 404/403, timeout, truncated network read, or invalid private locator never become missing or permission to delete; these remain UNKNOWN/errors pending independent distinction and retry.

No whole PACK URL is signed, no object repaired or overwritten, and no reader entitlement is inferred. The caller must be a trusted offline operator/worker and may not expose private keys, hashes or bodies to HTTP clients.

## Next gates, NOT completed here

1. Real disposable PG18 + MinIO full-GET tests: correct/corrupt/missing/short-read/forbidden/timeout, never classifying ambiguous S3 errors as missing.
2. Resumable **bidirectional** S3 LIST ↔ PG reconciliation with bounded memory, checkpoints, retry budgets, 24-hour PUT grace, staged PACK-direct, pending builds, saved FREE V1 and backup retention.
3. Sanitized orphan/unknown dashboards and a **dry-run only** report, with independent repeated evidence.
4. Separately approved DROP-07b GC: retention, holds, quorum, audit and concurrency proof. DELETE remains OFF.
5. Post-merge acceptance, DROP-08 independent recovery and DROP-09 disposable cloud-provider trials.

This PR only proves expected-object enumeration and bounded actual-byte validation. It does **not** prove object completeness or reclaimability.

[Original Notion plan](https://app.notion.com/p/41a32a84beec816db12deb24688716d6), DROP-07.
