# DROP-R3b — proof of entire immutable PACK before PostgreSQL promotion

**Status: G/B readback service and negative tests; no reader activation.** Builds on [R3a bounded private frame reader](tor-store-r3a-private-pack-range.md) and [R2 generation catalog](tor-store-free-placement-r2-pg.md).

## Required full-object acceptance protocol

1. B prepares a complete ordered inventory of exact immutable FREE editions: `(chapter_id,version_id,sha256,size_bytes)`. The source is immutable B data, not a browser-supplied manifest.
2. G uploads an immutable `private/free-packs/<...>.zst` and separately immutable `private/free-packs/<...>.manifest.json`. For a retry after uncertain PUT, **an independent GET of both complete objects** must verify the trusted SHA256 before any receipt is accepted. Their blob/checksum origins are untrusted before readback.
3. `S3PackProofStore::verify_independent_full_get` obtains both objects using only internal S3 credentials, with exact `Content-Length`, no `Content-Encoding`, a strict 16-MiB PACK cap and 1-MiB manifest cap. It does not sign or disclose a full-PACK URL.
4. `free_pack_proof::verify_full_pack` requires exact SHA256 of PACK + sidecar from trusted B/PG input, exact sizes, the standard Zstd seekable skippable magic/footer `0x184D2A5E/0x8F92EAB1`, frame count, encoded+decoded per-frame lengths, monotonically contiguous frames and seek-table start. It deserializes the sidecar into a strict typed schema with unknown/duplicate fields rejected. The format label `tor-store-public-seekable-zstd` is inherited from the experiment; **it does not imply public read access**: all S3 namespaces are private.
5. Each manifest entry is bound to exactly one B edition ID `chapter_id:version_id`. No missing/extra/duplicate/foreign edition, forged hash, misplaced frame or untrusted byte offset is accepted. Every independent Zstd frame is decoded within bounded output/window and checked against B's SHA256 and byte length.
6. Only after this full proof may a **separate B worker** insert a `free_storage_packs` receipt and candidate `free_edition_storage_generations`, conditioned on its valid current job lease/fingerprint and actor/source ACL in the *same* B-owned PG transaction. The G service returns verified physical frame references, not an entitlement or a publication capability.
7. Physical `promote_free_edition_storage` CAS is a later operation. Only the future C resolver and HTTP Range Gateway may use an active physical placement, after exact C permissions both before and after the I/O. Current S349 SINGLE read path remains authoritative until that separately accepted cutover.

## Code / tests

- `tor-storage::services::free_pack_proof::{PackProofTarget, verify_full_pack, VerifiedPack}`: pure full-PACK manifest and decoded edition proof with fail-closed error semantics.
- `tor-storage::repositories::free_pack_proof_s3::S3PackProofStore`: independent full S3 GET of two private objects, strict size/encoding limits, no presigns.
- Unit negatives: byte changes in PACK/seek-table/manifest, stale or missing/foreign B inventory, digest mismatch, forged frame offset even when attacker recalculates manifest SHA, overlong pack/manifest, invalid namespace.
- Extended `free_pack_range_s3` test: **real loopback MinIO** PUT both objects, independent full-GET verification before reading one authorized member via Range, saved rights recheck simulation, malicious post-proof S3 overwrite denied by decoded SHA and by full proof.

## Out of scope / remaining critical work

- No author/importer workflow performs the actual safe write/attach transaction yet. The R2 table `verified_at` is not independent proof by itself. A privileged SQL client can insert metadata without G proof; do **not** infer R2+R3b alone enforce safe production promotion.
- No real C `PgFreeS3Ref` permission wiring (age, hold, saved free pins, moderator and tenant boundary) to the PACK resolver; R3a's closure still uses test actors. No production HTTP Range endpoint, capacity limits, warm cache or monitoring.
- No paid ciphertext PACK. No crypto-shredding. No deletion/compaction until historical references and legal retention are proven.
- MinIO loopback tests are not a substitute for real Yandex S3 semantics and costs. No provider account, VPS, S3 configuration, production data or migrations are changed in this PR.

## Next acceptance

Exact-head Repository rules and full Integration CI, including `s3-provider` MinIO, Rust fmt/Clippy and negative tests. Then implement **R4 B job lease→verified G receipt→PG generation attach/atomic CAS**, followed by a real C-authenticated Range Gateway and access-denial Chromium tests before any feature flag can be enabled.
