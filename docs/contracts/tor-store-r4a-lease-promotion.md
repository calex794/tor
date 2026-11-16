# DROP-R4a — lease-fenced candidate PACK generation promotion

**Status:** additive PostgreSQL-only safety contract. **NOT an active PACK importer or reader.** R3b [#967](https://github.com/calex794/tor/pull/967) is merged and can independently read back/verify full immutable PACK + sidecar and every FREE edition, but no running importer currently calls that proof and commits its result.

## What is added

`migrations/202811164358_drop_pack_promotion_fence.sql` adds `free_pack_promotion_jobs` and an internal `finish_free_pack_promotion(job_id, epoch, fingerprint, pack_sha, manifest_sha)` function. Each job binds a frozen **exact B edition**, expected and next physical generation, immutable pack ID plus SHA256 for both object and sidecar, source bytes/digest, 32-byte approved inventory fingerprint, strictly increasing lease epoch and expiry. The job is not an authorization token.

Finalization takes the job row **FOR NO KEY UPDATE**, then the edition row (same lock discipline), checks current lease and every frozen hash, checks that the chapter/book are not held/deleted and remain FREE, checks the target immutable generation and pack hashes, and performs a single-edition compare-and-swap into `free_edition_active_storage`. It then records `succeeded` **within the same PostgreSQL function and transaction**. Any stale epoch/fingerprint, expired lock, hold, changed target, previous successful status or lost CAS returns FALSE. Two concurrent calls with the same expected epoch cannot both succeed.

No S3 network calls occur while any PostgreSQL lock is held. No original single-object reference, edition ID, reader pin or `chapter_publications` row is changed.

## Trust limitations — do NOT activate yet

- SQL cannot prove remote S3 bytes. The table's expected SHA256 fields and `verified_at` are **not** themselves a cryptographic attestation. A privileged database writer can forge them. R4b must supply an unforgeable internal call path from `S3PackProofStore::verify_independent_full_get`, with B's authoritative version inventory and verified lease ownership, and combine verified result with this finalizing transaction. Until then this function is an inactive internal candidate.
- Older R2 `promote_free_edition_storage` still exists; this new function does **not** revoke the older direct database primitive. Neither function is connected to any live HTTP/worker path. A later migration should remove or restrict direct use when enabling PACK.
- Each R4a job promotes **one** edition. A full multi-edition PACK requires R4b to coordinate atomically all expected B members and their independent saved entitlements and to retain preexisting SINGLE fallbacks.
- R3a `PackFrameStore` has no live C/PG ACL closure for saved V1, age, holds or subscription. No public HTTP Range Gateway, no protected signed PACK URL, no paid ciphertext migration, no automated GC.
- Source key is immutable; a future B/G worker must handle ambiguous PUT with verified GET, interrupted operations, retries and job lease loss, then fail closed when PG or provider is unavailable.
- No VPS, real S3 bucket, production migration or deployment is modified.

## PostgreSQL acceptance

Extended `tor-storage --features pg --test free_chapter_storage_pg` on disposable GitHub Actions PG18, using synthetic R2 editions/pack. The test verifies: wrong epoch, wrong fingerprint, wrong pack SHA, wrong manifest SHA, admin hold, expired lease, lease epoch renewal, two simultaneous finalizers where exactly one succeeds, no replay, immutable claim and terminal state, exact unchanged B publication V1.

**Next:** verify exact-head full Integration CI and Repository rules; then R4b real G readback + B inventory/lease→transaction workflow, followed by C authorizing Range Gateway/negative browser tests. Do not enable a PACK feature switch solely because these PG tests pass.
