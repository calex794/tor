# DROP-05c — explicit verified physical rollback (greenfield TOR)

Status: **Draft, disabled by construction**. This is an internal library primitive and a test-only coordinator. There is no public endpoint, admin action, background startup, live flag, or permitted production execution.

TOR has **no pre-existing catalogue to migrate**. This mechanism only safeguards future FREE editions first published as SINGLE, then deliberately promoted to an immutable PACK by DROP-05a. It does not touch PAID ciphertext or purchase grants.

## Operator recovery sequence

1. An authorized operator (future reviewed admin operation, **not implemented in this slice**) decides to restore an entire failed/undesired active PACK. Never automatically fallback on a Range/GET error.
2. G rereads **every** original S349 SINGLE from private S3 using its deterministic edition key, validates metadata, bounds, UTF-8 and SHA-256, and returns no raw bytes to the caller.
3. B accepts only the exact succeeded R4a job, immutable approval fingerprint, lease epoch, entire immutable membership, matching PACK identity and source receipts. It serializes one book and the exact active edition pointers under PostgreSQL locks.
4. B confirms **all** active members still point to the same exact PACK generations. In a single PostgreSQL transaction it inserts a **new strictly larger SINGLE generation** for each edition and CAS-promotes all pointers. There is no backward pointer update and no partial cutover.
5. A complete repeated operation is read-only (`AlreadyRolledBack`) for lost-commit-ACK recovery; a mixed/changed/incomplete batch fails closed. Both old PACK and SINGLE objects remain physically present for later independently approved DROP-07 inventory/GC.

## Rights and failures

- Author hold, deletion or access revocation does not become a reader grant. A held book can have its physical placement repaired while C's per-request authorization still denies the text.
- Missing/corrupt/unavailable original SINGLE stops *before* any PostgreSQL mutation, even when the PACK is already active.
- A changed active generation, partial rollback, fake manifest, stale epoch, absent immutable membership or edition/SHA mismatch denies the entire operation.
- No S3 delete, no signed PACK URL, no publication pointer changes, no new entitlement, no automatic switching on a transient PACK outage.

## Acceptance

- True disposable PG18 + private MinIO: two independent future free editions, verified activation from #1009, original SINGLE disappeared -> no rollback, restore exact source -> two-member higher-generation rollback, concurrency and lost-ACK replay, original PACK retained.
- PG checks enforce immutable edition identity, job fencing and monotone CAS; no partial states.
- Verify C/Chromium authorization with hold/pin/edit/progress separately before exposing any operator interface. Audit, metrics and a step-up/permission-checked operator control are **outstanding**.
- Exact-head CI then post-merge CI; no VPS deployment, productive migrations or live flags.

[Source plan in Notion](https://app.notion.com/p/41a32a84beec816db12deb24688716d6); dependency [DROP-05a / PR #1009](https://github.com/calex794/tor/pull/1009).
