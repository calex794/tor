# DROP-04a — verified PACK-direct staging (original Notion DROP plan)

**Status:** implementation under CI, no live import behavior changed. The original DROP-04 milestone includes **further** atomic batch publication and cutover; this PR only creates the durable, independently verified direct-PACK *stage*.

## Preconditions and trust chain

- The existing S023 source submission is fully checked by the G import worker: private source file, canonical UTF-8, expected count and SHA256/approved `fingerprint(book,chapters)`. The B `free_chapter_import_reservations` store immutable pending identities and UTF-16 content shapes, without writing chapter/version/publication rows.
- An internal-only `PgFreeAuthorPublisher::stage_direct_pack_only` processes those approved parsed chapters using reserved immutable IDs **directly into one Zstd seekable PACK**. There is no `S3FreeChapterStore::put`, no per-chapter SINGLE and no `free_chapter_storage_objects`.
- The key is deterministic from job book, actor, command key and approved content fingerprint. G writes the PACK and manifest with immutable conditional PUT, then performs an **independent full MinIO/S3 GET**, validating every frame and the exact manifest/seek table/content SHA. A lost PUT ACK is recovered only by complete readback, never by overwriting a key.
- Only after verified S3 proof does B `stage_verified_direct_pack` reauthorize the actual leased S023 job (actor, book, exact current epoch and wall-clock expiry, approved fingerprint and count), check the book is still an owned unheld private draft, and verify all reserved IDs, title, number, digest, UTF-8 size and ordinal position. Stage insertion is idempotent only for an identical receipt. A different receipt, stale lease, modified source or changed moderation state fails closed.
- New additive `free_pack_direct_import_stages` is immutable and **not** a free edition, publication, entitlement, `free_storage_packs` catalog entry, or active reader placement. No new reader API or PostgreSQL content body is created. The existing import worker still follows its accepted SINGLE path.

## Real isolated acceptance

PG18 + private MinIO test:
1. Create an actual approved import source and claimed, lease-fenced job for a private draft book.
2. Produce one private multi-frame seekable PACK from two different UTF-8 chapters, PUT and independently verify the exact remote PACK/manifest.
3. Replay the identical job and prove unchanged receipt and exactly one staged database row.
4. Assert **zero** per-edition SINGLE receipts, chapter publications and live chapters.
5. Reject altered source fingerprint and an expired/superseded lease without new publication.

## Original DROP-04 completion conditions still open

- A **separate** B transaction must write the exact imported chapter/version/content-shape/publication records and PACK generation/active pointer **atomically** with `chapter_import_jobs.status=succeeded`. Lost ACK retries and fencing must not duplicate physical generations.
- The C/G HTTP Gateway and browser must read active PACK editions before any PACK-direct source can become a visible chapter. Saved V1, age, hold, deleted book and arbitrary neighbor frames require negative acceptance.
- 100,000-chapter bounded import with resource/fault injection, orphan reconciliation and provider billing/latency tests (DROP-B03/B05/B06/B07/B08/B12).

No production deployments, source deletion, paid ciphertext, autonomous rollout or runtime migration. [Original plan](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
