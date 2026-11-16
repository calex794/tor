# TOR-STORE / DROP — free-edition physical-placement resolver v1

**Status:** non-activating code contract; **NOT** a deployed PACK reader, migration, authorization feature, paid-delivery change or verified storage object. `tor-storage::services::free_placement` introduces bounded choices and tests only. Related: [S349 direct free S3](free-s3-delivery-v1.md), [DROP benchmarks / #950](https://github.com/calex794/tor/pull/950), [DROP plan in Notion](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).

## The invariant

An immutable B chapter edition is identified by `(chapter_id, version_id, content_sha256, size_bytes)`. Physical location is a different concern. C still owns **all** publication, access, age, moderation, session, saved-version, and ordinary-deletion decisions. An S3 receipt or storage placement is **not** an entitlement.

A read is allowed only when C resolves the *exact* edition for the actor and the G physical record matches that same edition. After slow external storage work, C must recheck permissions and edition identity before returning a capability or bytes. PostgreSQL unavailability fails closed. Publishing V2 must never mutate V1's bytes or the reader's pinned V1 identity.

## Two deliberately unequal delivery paths

| Placement | Existing reader route | Security constraint |
|---|---|---|
| `S3_SINGLE` | Existing S349 private presigned GET (90 s), after C grant and G HEAD | The URL addresses exactly one edition's object |
| `S3_PACK` | **Future** authenticated, no-store server-side Range Gateway | Never presign the whole pack: a Range query or client-provided offset does not restrict a bearer URL to only the authorized chapter |

Only `FreeReadPlan::DirectSingle` exposes a canonical single-object reference; `FreeReadPlan::PrivateRangeGateway` contains a bounded internal frame reference. Neither value grants a right. There is **no** `S3_PACK` HTTP handler, live PostgreSQL selector, pack writer or GET in this PR; a caller must not silently treat the packed variant as a single-object fallback.

## Initial frame envelope

- Format candidate: **Zstandard seekable 0.1.0**, one independent compressed frame per edition, seek table at the end. Prototype: `tools/tor_store_seekable.py` on experimental #950; not a published Rust decoder.
- Physical object cap for the first controlled experiment: **16 MiB**. Per-frame compressed cap: `MAX_FREE_MARKDOWN_BYTES + 4096` (currently 204,096 bytes); exact target pack sizes 4–16 MiB are tuning parameters, not claims of an optimal size.
- A trusted future catalog must bind private `bucket`, `private/free-packs/... .zst` key, `pack_size`, `seek_table_start`, frame `offset/length`, and independent `pack_sha256` and `manifest_sha256` to the exact B edition. Offsets must be non-overflowing and within the frame data region, never the seek table.
- These structural checks do **not** verify S3 bytes or decoded text. The future G Range gateway must enforce HTTP 206 and exact `Content-Range` and length, decode with a strict output bound, and compare decoded `SHA256` and byte length against authoritative B metadata. The persisted pack manifest must be verified against its separately trusted PostgreSQL digest. No client-controlled S3 key or Range offset.
- A cache of the verified seek table may reduce to one Range GET; that shortcut is permissible only with independently proven pack immutability and a trusted manifest. Do not trust a mutable ETag or an unauthenticated manifest from S3.

## Migration and publication plan (NOT implemented)

1. Preserve the existing immutable `free_chapter_storage_objects` **single-object** record and its strict `private/free-chapters/{chapter}/{version}.md` key constraint. Do not change or reinterpret this S349 table in place.
2. Introduce separate, additive PostgreSQL **placement generations** and an active-generation pointer by exact edition. Publication/repoint is an atomic B/PG compare-and-swap after verified G receipt, not an S3-and-PG distributed transaction. Initial active placement remains `S3_SINGLE`.
3. For interactive author saves, keep single PUT/verified GET/publication as S349. For **bulk imports** write verified packs first, then atomically commit B editions and placement generations; do **not** require an expensive single-object PUT for every imported chapter. This requires a new batch-publication contract and tests; it does **not** work with the current S349 single-object trigger unchanged.
4. Implement server Range Gateway with the *actual* C authorization predicates, hold/session revocation checks, anti-cross-tenant negative tests, and fail-closed behavior when PostgreSQL or S3 is unavailable. A pack's neighboring chapters must remain inaccessible even to a user who has one chapter pinned.
5. Switch a single test edition to PACK through B's compare-and-swap. Preserve prior generations until all in-flight requests and existing short-lived single-object URLs expire, and until pin/rights/reference checks allow garbage collection. A downgrade from PACK to SINGLE needs the same atomic/verified contract.
6. Only after real PG18/MinIO/Chromium acceptance should a small feature-gated cohort use PACK. Keep the current direct S349 path as default. Rollout/rollback and real-provider compatibility require separate approval; do not deploy or apply migrations during this experiment.

## Safety acceptance before enabling

- Exact B V1 remains readable for its authorized pin when V2 is published, free-to-paid conversion occurs, or ordinary deletion takes effect; hard holds and age revocations deny **new** reading.
- A signed GET for a PACK object is **never** sent to the browser, even if the object has one readable frame and all others are free today. Every frame is independently authorized.
- Wrong chapter/version/hash/size and replayed or torn manifest => reject. Overlong/overlapping/overflow ranges, short Range responses, and decompression bombs => reject with no plaintext.
- Concurrent repack, compaction, cancelled import lease, partial S3 PUT, lost COMMIT ACK, and PostgreSQL outage cannot create a live pointer to missing or unverified bytes.
- Original immutable editions and pinned references are not deleted by compaction. Orphan and dead-byte metrics are computed, but deletion is a distinct reviewed, retention-aware operation.
- Same-compression baselines and realistic mixed reads must be measured for cost/latency. Experimental #950 localhost results cannot justify actual provider bills.

**Explicitly outside the current stage:** paid ciphertext and purchased-entitlement migration, S3 object GC, background rewriter, encrypted frame scheme, domain routing, production migration, VPS rollout and real S3 changes.
