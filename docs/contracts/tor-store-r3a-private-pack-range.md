# DROP-R3a — bounded private PACK frame reader (inactive)

**Status: source-code prototype; no public HTTP route or production switch.** Based on [DROP-R1 placement plan](tor-store-free-placement-v1.md), and designed to consume the [DROP-R2 PostgreSQL generations](tor-store-free-placement-r2-pg.md) only after they receive separate acceptance. This PR does not depend on R2 merging to compile.

## Implemented

- `tor-storage::services::free_pack_read`: trait `PackFrameStore`, `decode_exact_frame`, `read_reauthorized_pack`.
- Strict `zstd` **one-frame** decoder: validate exact compressed frame size using `zstd_safe::find_frame_compressed_size`, reject concatenated frames, cap output to the B edition byte size (at most 200,000 bytes), independently compare SHA256 and canonical UTF-8.
- `read_reauthorized_pack` requires the caller to supply fresh exact C authorization + identical placement snapshots **twice**, before the S3 call and after decoding, before returning plaintext. A hold/revocation/PG outage or changed edition/placement yields **no plaintext**.
- `tor-storage::repositories::free_pack_range_s3::S3PackFrameStore`: server-only S3 `GET Range` derived exclusively from validated placement metadata; never a presigned URL. Requires exact `Content-Range: bytes start-end/total`, exact Content-Length and no unexpected Content-Encoding, with bounded response body. An optional metadata SHA256 disagreement fails closed. A full-object 200 ignoring Range cannot pass.
- Unit tests: authorized chapter in a two-member pack is the **only** decoded output; forbidden initial request cannot fetch; single object cannot be mistaken for PACK; revocation and PG outage after fetch suppress text; changed edition/generation, truncated/damaged/concatenated frames, wrong digest, oversized decoded text and S3 outage fail closed.
- MinIO E2E fixture `crates/storage/tests/free_pack_range_s3.rs`: actual independent Zstd frames and standards-compatible seek table, private object PUT, anonymous GET denied, legitimate internal Range returns only authorized frame, reauthorization denies later hold, and test-only object tampering cannot yield accepted plaintext. Wired into `tor-integration.yml` and `storage-s3.yml` s3-provider lanes.

## Exact limits / unresolved trust boundaries

This is **R3a** rather than the complete R3:

1. It accepts a validated `PackMetadata` snapshot but does **not** independently fetch or validate the sidecar manifest against the trusted PostgreSQL manifest SHA256. The G publisher/ingester must verify full immutable S3 PACK + seek table + sidecar against the B receipt before any generation may become active. A frame-only Range GET cannot prove the whole-PACK digest on every read.
2. The resolver closure is provided by the future caller: this PR does not yet wire that closure to the real `PgFreeS3Ref` policy, saved pins, age, holds, tenant boundary and access checks. A synthetic test reauthorization is not production proof.
3. No real HTTP route, rate limits, memory/CPU worker cap, request cancellation, cache rules, observability, physical promotion, or writer. No production PACKAGE read, no shared signed URLs, no reader-facing raw PACK offset.
4. Paid encrypted text uses a separate entitlement and decryption model. None of this code may silently become the paid reader.
5. The synchronous decoder has a hard output bound; before an HTTP endpoint is connected it also needs bounded `spawn_blocking`/CPU concurrency and read deadlines.
6. `FreeChapterObject::sha256` is authoritative per-edition metadata, not the optional S3 user-metadata hash. A changed/shortened S3 Range must fail closed even if an S3 ETag appears valid.

## Acceptance before a real gateway

- Exact-head Rust fmt/Clippy and all integration jobs.
- Real MinIO `free_pack_range_s3` PASS, including Range and anonymous denial.
- Follow-up R3b: independent trusted-manifest parser/checker; B verified receipt, real C saved/age/hold recheck, and no-store HTTP route returning decoded text only after final authorization.
- Follow-up R4: atomic PG single→pack promotion and rollback with old V1 pinned and bodyless PG intact; no accidental blind whole-pack URL.
- No real-provider/VPS/S3 deployments, no migration execution outside disposable CI.
