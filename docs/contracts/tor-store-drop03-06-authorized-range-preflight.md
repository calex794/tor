# DROP-03/06 — C-reverified private PACK Range preflight (no live reader route)

This is the next missing safe-public-PACK/Range component in the **original Notion DROP-00…DROP-09 plan**, section 17 and the vertical slice ordering in section 19. It does not replace the original DROP-04 pack-direct bulk importer or the DROP-05 automatic SINGLE→PACK migration.

## Original-plan boundary

1. **C authority:** reuse the exact S349 `PgFreeS3Ref::for_session` and `for_saved_session` SQL authorization for the chosen chapter and immutable version (public visibility, age levels, session, saved pin, moderation holds, deleted book semantics).
2. **B provenance:** `PgFreePackCatalog::verified_free_range_candidate` returns physical metadata only for a candidate whose R4a job is `succeeded` after independent G full PACK/manifest verification. Exact `(chapter,version,SHA256,size,bucket)` must match the current C-authorized reference. No user-supplied manifest, offset, length or PACK object name enters this lookup.
3. **G exact Range:** `S3PackFrameStore` issues a private server-side S3 Range GET of the chosen compressed frame. It checks exact HTTP `Content-Range`, content length and bytes, then `decode_exact_frame` bounds Zstd CPU/window/decoded bytes and verifies canonical UTF-8+SHA256. No signed PACK URL is returned.
4. **Second C/B check:** `read_reauthorized_pack` resolves the same exact authorized reference again after S3 I/O; any revoked grant, changed publication/edition, changed placement or PostgreSQL outage fails closed with **no returned chapter bytes**.
5. **No cutover:** `read_authorized_candidate` is intentionally **internal, not registered with Axum**. S349 SINGLE remains the sole live reader. This stage provides the evidence needed before the original plan's HTTP Gateway acceptance and later, separate feature-gated CAS promotion.
6. **Fallback and private end-to-end:** disposable PG18/MinIO acceptance creates real FREE SINGLEs, successfully attaches an independently verified two-frame PACK, reads *only* each edition's requested bytes via Range and checks hold/saved-denial. The already accepted R4b1–R4c tests continue to prove immutable sources and one-winner fenced attachment.

## Scope limitations

This is a preflight for **FREE** editions only. It does not add paid ciphertext or publish a bookmark/saved-V1 UX route. The original DROP-04 PACK-direct ingest remains required for bulk-import cost savings. Neither this change nor R4c activates any reader, deletes SINGLE or runs a production migration.

### Next steps under the original plan

- Finish C/G HTTP Range Gateway tests including saved free V1, age/hold/deletion, denial of other frames and browser behavior (original DROP-03/06).
- Implement original DROP-04 staged **PACK-direct** bulk ingest using the existing S349 leased queue; atomic finalize, repeated claims and upload/commit crash recovery.
- Finish DROP-05 CAS promotion/fallback; DROP-07 inventory/GC; DROP-08 DR; DROP-09 provider acceptance. Preserve all DROP-B01…B12 benchmarks (including Single compressed and actual provider costs).

[Original Notion plan](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
