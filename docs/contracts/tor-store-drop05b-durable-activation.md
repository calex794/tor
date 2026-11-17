# DROP-05b — durable, lease-fenced automatic PACK activation (greenfield TOR)

**Implementation status:** Draft awaiting exact-head CI and post-merge. The runtime path is implemented but is **OFF by default**; it is not deployed or production-approved. TOR has no existing catalogue to migrate. Only future FREE SINGLE editions with verified R4a PACK candidates participate. PAID data and historical backfill are out of scope.

## 1. Durable boundary between candidate and active placement

R4a commits the complete physically verified PACK as an inert candidate. The old R4b3 worker does not reclaim jobs after they are `succeeded`; a one-shot postcommit activation would be lost after a crash.

The additive migration `202823657191_drop_pack_activation_queue.sql` creates `free_pack_activation_jobs`. Its PostgreSQL trigger enqueues one immutable row in **the same transaction** that changes the R4a PACK job to `succeeded`. On rollback neither the candidate nor the activation exists; on commit both exist. A lost acknowledgement cannot create duplicate queue rows.

An activation job has pending/running/succeeded/failed state, a **separate** monotone lease epoch, at most eight attempts, bounded retry backoff, and a terminal failure code. The R4a build lease remains immutable after success. A read-only schema probe fails closed if the additive migration is unavailable.

## 2. Verified G proof and fenced B commit

`DropPackActivationWorker` claims one queued job via `SKIP LOCKED`, renews its lease while waiting for external S3 and never holds an application PostgreSQL transaction over S3 I/O. An expired/crashed worker is reclaimable at a newer activation epoch. Parent R4a jobs are locked before activation rows, including cleanup, to avoid inverted-lock deadlocks.

Before B cutover, G performs bounded private GET + exact SHA/UTF-8 verification of **every original SINGLE**, and separately reads the entire PACK plus immutable manifest to verify each frame. A corrupt immutable object is terminal; a temporary provider outage is retryable. The original SINGLE is never deleted.

B's `activate_queued_verified_pack` validates the exact succeeded R4a fingerprint, member inventory, book/owner/hold, original receipts and candidate frames, then locks the **current activation epoch and unexpired lease inside the same PG transaction** that CAS-promotes all edition active pointers and marks the queue `succeeded`. Any revoked, stale, mixed or partial state rolls the whole transaction back. There is no independent `Completed` settlement path.

Consequently, interruption before B leaves active SINGLE; interruption after B commit leaves every member active as PACK with a succeeded queue marker. A lost COMMIT ACK is a read-only replay. A heartbeat blocked by the B row lock has a bounded wait, allowing the same task to finish its B future.

## 3. Operator gating

The independent `TOR_DROP_PACK_CUTOVER_ENABLED=1` flag is required in addition to authenticated PostgreSQL, private FREE S3, an enabled R4b3 candidate worker and the server-side PACK reader. It is rejected in demo mode or a build without `s3-free`. The startup probe checks both worker and activation schemas. Default is **0 / OFF**. Candidate building, planning, mass-import PACK-direct and PACK read routing remain separate controls.

No public API, author-controlled promotion, signed whole-PACK URL, reader entitlement change, S3 DELETE, automatic fallback on reader failure, VPS deployment, production migration or external bucket access is introduced by this PR.

## 4. Tests and acceptance gate

Disposable PG18+MinIO coverage includes R4a candidate-commit/queue atomicity, claim and stale lease rejection, source corruption and missing SINGLE denial, independent full PACK proof, expired or forged activation epochs, concurrent manual/queued activation, all-member CAS, idempotent replay, retained original SINGLE and explicit separate PACK→SINGLE recovery from DROP-05c. Flag tests reject every missing dependency.

Before acceptance: exact-head CI including PostgreSQL foundation and real MinIO + browser privacy regression must pass **23/23**, followed by the same complete post-merge CI on the new `main`. Failed or cancelled runs are not evidence of acceptance.

## 5. Remaining in the original plan

- DROP-05 operational work: sanitized metrics, alerts, and an explicitly authorized reset workflow for terminally failed activation jobs; no destructive action.
- DROP-07: independent FREE SINGLE/PACK S3↔PG inventory, protected V1 references, orphan reconciliation and separately approved retention-based GC. Existing G-09 primarily covers private encrypted objects and is not proof of FREE PACK inventory.
- DROP-08: independent backup/restore drills and measured RPO/RTO.
- DROP-09: test-only real provider acceptance and a separate security-reviewed rollout decision.

[Original Notion plan](https://app.notion.com/p/41a32a84beec816db12deb24688716d6). [PR #1014](https://github.com/calex794/tor/pull/1014).
