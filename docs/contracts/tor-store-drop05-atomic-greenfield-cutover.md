# DROP-05a — atomic activation of verified SINGLE-to-PACK candidates (greenfield TOR)

Status: implementation contract; runtime remains OFF until PG18/MinIO/Chromium exact-head acceptance.

## Scope and baseline

TOR starts from zero: **there is no existing book catalogue to migrate**. This stage is *not* a historical backfill. It handles future FREE S349 editions first published as SINGLE and later eligible for opportunistic PACK compaction. Mass imports already have DROP-04 PACK-direct. Interactive edits intentionally produce immutable SINGLE V2, leaving pinned PACK V1 intact.

Existing components: R4c schedules verified SINGLE sources, R4b3 writes and re-proves one immutable Zstd PACK and R4a attaches inert candidate generations, DROP-06b reads only authorized active PACK. No R4a worker or HTTP actor may activate a candidate as a side effect of upload.

## Trusted cutover interface

- Explicit opt-in `TOR_DROP_PACK_CUTOVER_ENABLED` defaults OFF, requires authenticated PG, private S3, enabled PACK reader and verified worker; no automatic migration on startup.
- G independently re-reads PACK and manifest and checks every immutable frame and sha before B is invoked. An old receipt, a client-provided manifest, or a successful S3 PUT alone is insufficient.
- B processes **all** members of exactly one succeeded R4a job in **one PostgreSQL transaction** under a per-book serializing lock and immutable exact membership. This is all-or-nothing, never partial per-frame promotion.
- B validates owner/account/book/hold/deletion, exact FREE edition ID/version/hash/size, immutable source SINGLE receipt and pin rights, matching bucket/pack/job/member, currently active source generation and immutable candidate generation.
- B CAS-promotes only if *every* target is currently the expected exact SINGLE generation. If any is already PACK, moved to a different generation, lacks exact source, is held or has a changed edition, abort the entire batch with no updates; an identical lost-commit-ACK replay must return a verified already-completed result instead of producing a second generation.
- Concurrent workers, expired leases and stale epochs cannot promote; the approval/proof token is bound to the exact job, generation inventory and current fencing ownership. A fresh independent proof is required after reclaim.
- The active storage pointer and audit of the expected→next transition commit atomically. Generation numbers are strictly increasing, including rollback. No committed reader ever sees a partially switched batch.
- Source SINGLE bytes and receipt remain intact after promotion; **no deletion** belongs to DROP-05. Reconciliation of orphans and retention is DROP-07. No direct signed PACK URL, change to entitlement, or paid-content conversion.
- Rollback is a separate explicit action requiring a new full GET + SHA check of *every* original SINGLE, then a new higher SINGLE generation and atomic CAS; no pointer rewind, no implicit fallback on a PACK GET error.

## Test acceptance (disposable PG18 + private MinIO + Chromium)

1. Two independent FREE revisions and two books, exact selected batch and no adjacent-frame leakage.
2. POST-upload pre-proof outage, lost PUT ACK, corrupted manifest, short GET, checksum mismatch: zero active changes.
3. All-or-nothing PG injection at first and later member; lost PG COMMIT ACK and exact replay; concurrent cutover with two workers.
4. Changed owner, actor, book hold/delete, age policy, version/hash mismatch, changed active generation and reclaimed lease each deny cutover.
5. Saved pinned V1 remains readable after V2 SINGLE publication; owner editing, progress UTF-16, comments and browsing remain correct on desktop/mobile.
6. PACK reader failure is fail-closed; operator rollback only with independently verified SINGLE and strictly newer generation, restores all members together.
7. Original SINGLE receipts still exist, no S3 delete, no direct PACK URL, no fake SINGLE and no plaintext in PostgreSQL.
8. Exact-head CI and post-merge CI succeed before stage accepted; no VPS deployment, production migration, or live flag change.

## Work slices

- DROP-05a: B transaction and generation/lease CAS with PG concurrency and lost-ACK tests.
- DROP-05b: G proof orchestration, bounded worker, opt-in bootstrap and PG18/MinIO fault tests.
- DROP-05c: explicit rollback with new SINGLE generation, browser and rights acceptance, metrics/alerts.
- DROP-07 remains responsible for inventory, orphan reconciliation and physical GC.

[Original Notion plan](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
