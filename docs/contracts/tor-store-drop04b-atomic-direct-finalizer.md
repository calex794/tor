# DROP-04b — atomic direct PACK batch finalizer (original DROP-04)

**Status: PR acceptance pending. No live importer or reader cutover.**

The already accepted DROP-04a stages an independently proven private seekable PACK and reserves immutable chapter/version IDs, but does not publish chapters. This stage supplies the missing **single PostgreSQL transaction** to materialize the authorized batch from that proof. It is a component of the original DROP-04 pack-direct milestone, **not its full rollout**.

## Mandatory data and trust boundaries

1. **Fresh G proof:** internal `finalize_direct_pack_after_reproof` first reads the *entire* private S3 PACK and manifest independently and checks every member against the approved canonical chapter text, SHA256, seek-table frame and exact version ID. There is no direct HTTP call, untrusted S3 manifest-to-PG attach or client-selected range. The method is not wired into the S023 live worker.
2. **One B transaction:** lock the exact S023 `chapter_import_jobs` row; check current lease epoch and wall-clock expiry, actor/book/command, approved source fingerprint/count; lock the still-owned private draft book; compare the immutable `free_pack_direct_import_stages.receipt` with that fresh proof; compare all `free_chapter_import_reservations` ordinal IDs, titles, chapter numbers, content digests and sizes. Refuse mixed SINGLE upload checkpoints or number conflicts.
3. **All-or-nothing materialization:** insert one immutable `free_storage_packs` row, the complete `chapters`, bodyless `chapter_versions`, immutable UTF-16 shapes, per-edition `free_edition_storage_generations` PACK frames, and CAS-promote each first `free_edition_active_storage` generation. Insert all `chapter_publications`, record the immutable `chapter_import_batches` command, and mark the exact leased S023 job `succeeded` with saved_count in the *same transaction*. If any check/trigger fails or the lease expires, rollback **all** of these writes. Do not create any per-chapter SINGLE object.
4. **Keep the SQL publication invariant:** an additive `CREATE OR REPLACE FUNCTION guard_s349_free_publication` allows a bodyless free publication with either (a) the original exact S349 SINGLE receipt, or (b) an exact active PACK generation whose parent pack has a matching immutable verified direct-import stage. Anything else continues to fail with SQLSTATE 23514. Existing single-backed books and paid ciphertext are untouched.
5. **Replay and revocation:** on a lost PostgreSQL COMMIT acknowledgement the identical succeeded batch is accepted only after checking the same staged proof, immutable chapter batch fingerprint and the complete count of active PACK publication records; altered receipts, stale epochs, revoked book ownership or holds before commit fail closed. Do not accept a bodyless chapter with neither SINGLE nor approved PACK.

## Isolated proof

The new ignored PG18+MinIO integration test extends `drop_04a_pack_direct_pg_minio`: starts with an S023 approved job, G verifies a real remote two-frame PACK, B rejects a forged lease and a held book with zero new chapters, then finalizes all editions together; verifies one immutable PACK, two exact active generations with UTF-16 shapes, zero SINGLE receipts, and one succeeded job and batch. Replays an identical completed commit, rejects an altered receipt, and exercises the strengthened S349 refusal of an unplaced bodyless chapter.

## Out of scope for this acceptance

The live importer remains SINGLE-first. The live /free-grant endpoint continues to sign only an authorized single-file reference and **cannot** serve PACK-direct editions; this finalizer is deliberately internal-only and is never invoked from a live request or worker. **Before opt-in** the original DROP-06 HTTP Range Gateway must authorize exact ACTIVE PACK versions, validate saved V1 and age/hold/delete, recheck after S3 GET, and provide the client with a compatible text-only response. Only after that can a feature flag route S023 job claims into direct PACK, with retry/crash tests, resource bounds and 100k chapter benchmark. No live PG migrations, VPS deployment, signed PACK URLs, S3 deletion or paid-text changes.

[Notion Дроп](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
