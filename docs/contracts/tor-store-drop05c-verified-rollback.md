# DROP-05c — explicit verified physical rollback (greenfield TOR)

Status **isolated operator drill**. The command accepts a disposable local PostgreSQL database and a dedicated loopback MinIO bucket.

TOR has **no pre-existing catalogue to migrate**. This mechanism only safeguards future FREE editions first published as SINGLE, then deliberately promoted to an immutable PACK by DROP-05a. It does not touch PAID ciphertext or purchase grants.

## Operator recovery sequence

1. The operator chooses the succeeded Single→PACK job and its exact build epoch. The command loads the approved receipt and validates its fingerprint before reading the sources.
2. G rereads **every** original S349 SINGLE from private S3 using its deterministic edition key, validates metadata, bounds, UTF-8 and SHA-256, and returns no raw bytes to the caller.
3. B accepts only the exact succeeded R4a job, immutable approval fingerprint, lease epoch, entire immutable membership, matching PACK identity and source receipts. It serializes one book and the exact active edition pointers under PostgreSQL locks.
4. B confirms **all** active members still point to the same exact PACK generations. In one PostgreSQL transaction it creates higher SINGLE generations, advances every pointer and records the job, PACK, epoch, database session role and commit time.
5. A complete repeated operation is read-only (`AlreadyRolledBack`) for lost-commit-ACK recovery; a mixed/changed/incomplete batch fails closed. Both old PACK and SINGLE objects remain physically present for later independently approved DROP-07 inventory/GC.

## Rights and failures

- Author hold, deletion or access revocation does not become a reader grant. A held book can have its physical placement repaired while C's per-request authorization still denies the text.
- Missing/corrupt/unavailable original SINGLE stops *before* any PostgreSQL mutation, even when the PACK is already active.
- A changed active generation, partial rollback, fake manifest, stale epoch, absent immutable membership or edition/SHA mismatch denies the entire operation.
- No S3 delete, no signed PACK URL, no publication pointer changes, no new entitlement, no automatic switching on a transient PACK outage.

## Acceptance

- True disposable PG18 + private MinIO: two independent future free editions, verified activation from #1009, original SINGLE disappeared -> no rollback, restore exact source -> two-member higher-generation rollback, concurrency and lost-ACK replay, original PACK retained.
- PG checks enforce immutable edition identity, job fencing and monotone CAS; no partial states.
- The PG18+MinIO case checks exact job lookup, rejects a different epoch or bucket, and records one receipt across simultaneous rollback calls and replay.
- Production operator access and alerts require a separate reviewed rollout. Complete C/Chromium checks for hold, pins, edit and reading progress on the staged environment.
- Exact-head CI then post-merge CI; no VPS deployment, productive migrations or live flags.

[Source plan in Notion](https://app.notion.com/p/41a32a84beec816db12deb24688716d6); dependency [DROP-05a / PR #1009](https://github.com/calex794/tor/pull/1009).

## Run the local drill

On the disposable stand, initialize the migrated `tor_drop_rollback_drill`
database or use the isolated `tor_ci` fixture. Activate one verified
Single→PACK job backed by a `tor-rollback-drill-*` bucket. Use its exact
job ID and build epoch. Provide `TOR_DROP_ROLLBACK_DATABASE_URL`,
`TOR_DROP_ROLLBACK_S3_ENDPOINT`, `TOR_DROP_ROLLBACK_S3_BUCKET`,
`TOR_DROP_ROLLBACK_S3_ACCESS_KEY` and `TOR_DROP_ROLLBACK_S3_SECRET_KEY`
through the drill environment.

```sh
TOR_DROP_ROLLBACK_DRILL=YES cargo run -p tor-web --features s3-free \
  --example drop_pack_rollback_drill -- "$job_id" "$build_epoch"
```

Repeat the command to confirm `AlreadyRolledBack`, then check the single
receipt and the active generation of every member. The archive and
original SINGLE objects remain available throughout the drill.
