# DROP-R2 — additive PostgreSQL catalog of immutable storage generations

**Status: implementation proposal and native PG18 CI test. NOT a live rollout or authorization implementation.** Depends on S349 (merged) and [DROP-R1](tor-store-free-placement-v1.md) (merged #960).

## Problem

A single chapter edition `(chapter_id,version_id,content_sha256)` must survive a physical move between one standalone S3 object and a packed, compressed S3 object. The reader's publication pointer and saved-free pin must remain stable even while the old object remains available. PACK cannot be distributed as a whole-object S3 presigned URL, because a reader with the URL could select a neighboring Range.

## Additive schema

Migration: `migrations/202811163749_drop_free_placement_generations.sql`.

| Table | Purpose | Invariants |
| --- | --- | --- |
| `free_storage_packs` | immutable verified-on-ingest S3 pack receipt; private bucket, immutable key, full PACK and manifest SHA256, bounded pack length and seek-table start | no UPDATE/DELETE; external G GET evidence still needed |
| `free_edition_storage_generations` | physical generation for the same immutable B edition; either `single` or `pack` with frame offset/encoded bytes | composite FK to B edition; chapter must be FREE when inserted; exact source SHA; `single` requires existing S349 receipt with exact byte count; PACK requires immutable pack and bounded, non-overlapping compressed frame |
| `free_edition_active_storage` | generation cursor per exact edition, **not** a reader permission/publication pointer | composite FK to existing generation; cannot downgrade or DELETE; no initial rows created by migration |

B/G may eventually promote a generation through `promote_free_edition_storage(chapter, version, expected_generation, next_generation)`. It locks the exact edition row and uses compare-and-swap. No initial pointer means expected generation **0**; later changes must strictly advance. A cancelled/stale worker trying to use a previous generation returns **false**. The old metadata and S349 SINGLE receipt remain unchanged.

**Important limitation:** the SQL function is an internal catalog primitive, not a trust boundary that independently proves the remote bytes. It does **not** take an actor session, payment entitlement, lease epoch or full approved source fingerprint. B may only call it after a separately verified G receipt, existing author/worker checks and current job lease/fingerprint, all within one B-owned transaction. Do not expose this function as a generic HTTP endpoint.

## Correctness and security boundaries

1. Never reinterpret the S349 `free_chapter_storage_objects` key as PACK. Its canonical exact-single `private/free-chapters/{chapter}/{version}.md` constraint and immutable receipts remain untouched.
2. Never use `free_edition_active_storage` to authorize a reader. C's exact-version PG permission checks, saved pins, age and hold checks remain mandatory and must be repeated after the external S3 call.
3. Packing and compaction cannot alter an edition's ID, version hash, source shape or existing reader pointer. The migration does not backfill, move or delete any object.
4. A PACK generation is structurally checked against private S3 namespace, outer pack bounds, seek table, non-overlapping frame intervals and B content hash. **Actual pack integrity is not proven by SQL**; G must read/check the physical S3 bytes, independent Zstd frame sizes, decoded SHA256, canonical manifest digest, and signed provider responses.
5. For this R2 migration **the existing S349 reader ignores all three new tables**, irrespective of a generation promotion. A hypothetical active PACK cursor still must NOT cause a browser presign or a different response.
6. Because a free chapter could later be purchased or pinned, do not infer eternal public access. The future Range Gateway must read only the caller's authorized frame and never issue a capability for the entire pack.
7. Catalog compaction and orphan GC must be separate, retention-aware operations. A deleted book does not imply previously acquired/allowed editions can be erased.

## Testing

Existing `tor-storage --features pg --test free_chapter_storage_pg` is run by the real disposable PG18 `postgres-runtime (foundation)` Integration CI after applying all migrations to a throwaway `tor_ci` twice.

The added `drop_r2_pack_placement_generations_are_immutable_and_cas_fenced` test verifies:
- Exact original S349 SINGLE identity survives metadata switches.
- Only free editions can obtain free placement generations; paid editions cannot be inserted.
- Wrong hash/byte length and overlapping or overlong frame are rejected.
- PACK and generation rows cannot be mutated/deleted, and active generation cannot rewind.
- CAS initial 0→1, forward 1→2, retries/stale requests and two concurrent workers promoting 2→3 / 2→4: exactly one succeeds.
- B's published version pointer never changes when only physical placement changes.

### NOT yet covered

No real S3 PACK range GET, no Zstd Rust decompression, no physical remote readback, no full S349 saved-pin/author/paid integration through an operational Range Gateway, no actual bulk import, no interrupted S3→PG production writer, no replica/restore or cloud RTT. **Do not enable PACK reading before R3/R4 and exact-head CI acceptance.**

**Operational rule:** work in repository only; migration is added to source and applied only in disposable GitHub Actions, **not** on VPS or real PostgreSQL without a separate rollout decision.
