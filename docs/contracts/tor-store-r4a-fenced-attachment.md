# DROP-R4a — lease-fenced B attachment of verified physical PACK generations

**Status:** candidate-only PostgreSQL transaction, no live reader switch, no production worker or migration rollout. Source: [R2 schema](tor-store-free-placement-r2-pg.md), [R3a Range reader](tor-store-r3a-private-pack-range.md), [R3b entire-PACK proof](tor-store-r3b-full-pack-proof.md).

## Dependency sequence and non-negotiable invariant

1. A separate B scheduler records an approval for **one book** and a complete ordered list of immutable FREE editions, the exact target private PACK+sidecar keys and their planned content SHA256 and sizes; the stable versioned `PgFreePackCatalog::fingerprint(target, editions)` is stored in `free_pack_jobs.approved_fingerprint`. It claims an epoch/expiring lease. The scheduler and claim API are **not** implemented in R4a.
2. A G worker uploads immutable `*.zst` and matching `*.manifest.json` under private S3 credentials and calls `S3PackProofStore::verify_independent_full_get`. No user gets a presigned URL for the whole pack.
3. G returns the opaque, unforgeable-by-normal-construction Rust `VerifiedPack`, with exact `PackProofTarget` and fully verified individual frame references. **Important nuance:** `verify_full_pack` is also a public pure verifier and could be called with local bytes by trusted internal code. Receipt safety depends on correct server orchestration; the Rust type alone is not a remote-S3 attestation.
4. B calls `PgFreePackCatalog::finish_pack_job(job_id, lease_epoch, &verified)` and enters **one** PostgreSQL transaction: locks the specific job, checks running/current/live lease and approved fingerprint/count, locks the owner book and every chapter in canonical order, confirms each exact immutable B `(chapter,version,sha,size)` and original S349 SINGLE receipt, then inserts one immutable `free_storage_packs` row and all immutable `free_edition_storage_generations` candidates and marks the job succeeded. Any error rolls back everything.
5. If the DB committed but the app lost the COMMIT reply, retrying with the **same** successful epoch/fingerprint returns the original `pack_id`, without creating another generation. Wrong epoch, expired/cancelled lease, book admin hold, other-book edition, paid chapter, altered source/size or missing standalone S349 receipt is a conflict.
6. **No active generation pointer is modified**: `free_edition_active_storage`, `chapter_publications`, saved reader pins, S349 presigned direct-object URLs, and the original `free_chapter_storage_objects` remain unchanged.
7. Pack objects left on S3 by a failed B transaction are orphans for a separate, retention-aware cleanup. Never delete an original SINGLE V1 merely because a compressed candidate exists.

## Files

- `migrations/202812117383_drop_pack_jobs.sql`: independent `free_pack_jobs`, not a repurposed chapter import job. Unique approved fingerprint per book, lease epoch/status and expiration, S3 candidate pack reference and terminal state. No migration backfill.
- `crates/storage/src/services/free_pack_proof.rs`: `VerifiedPack` now retains the exact trusted proof target and frames, still with private construction.
- `crates/books/src/repositories/free_pack_attach.rs`: pure B-owned transaction (no network), stable fingerprint, full batch atomicity, epoch fencing and idempotent retry.
- `crates/books/tests/free_s3_lifecycle_pg.rs`: new real PG18 `drop_r4a_verified_pack_is_lease_fenced_and_keeps_single_reader_unchanged`. Creates two actual B/S349 immutable editions, verifies a standards-compatible Zstd PACK, checks bad epoch, expiration, cancellation, forged fingerprint, author hold, concurrent exact replay and unchanged historical single receipts/publications.

## Explicit limitations and acceptance still missing

**No actual pack writer, queue claimer, import-only PACK path, C permission resolver, HTTP Range gateway or active-epoch cutover** in R4a. Tests fabricate synthetic private bytes and validate them with the R3b pure verifier; independent S3 full GET remains separately proven in the R3b real MinIO lane. The end-to-end **G independent full GET → B job claim → PG transaction** will require a separate test involving both real MinIO and real PG, and cannot be claimed merely from isolated unit/PG tests.

Direct B dependency on G's **proof type** is intentional; B never depends on S3 client, holds a network connection, or issues a network request. Before operational activation, audit that no other crate bypasses the verified G path and that the service role cannot submit arbitrarily approved receipts.

No paid-ciphertext PACK or migration of bought V1. No production rollout, VPS, real S3, real PostgreSQL migrations or destructive changes.

## Next stage

R4b: actual scheduler/worker lease claim and independent readback handshake, replay-safe retry after uncertain S3 PUT; make a single PG18+MinIO acceptance proving all-or-nothing candidate attachment after an S3 outage, lease theft, hold and source edit, with no active pointer or reader leak. Only then R5: real C-authorized, no-store Range Gateway, and later controlled physical CAS cutover with rollback.
