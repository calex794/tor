# DROP-05b — durable automatic PACK activation queue (greenfield TOR)

Status: **first implementation slice, Draft. No runtime activation.**

Original plan: [Notion — Дроп](https://app.notion.com/p/41a32a84beec816db12deb24688716d6), DROP-05. TOR has no existing catalogue: this supports only future FREE SINGLE editions, built as verified R4a/R4c PACK candidates. Historical backfill and PAID data are out of scope.

## Crash gap closed by the additive queue

The accepted R4b3 worker first commits physical PACK and changes its R4a job to `succeeded`. That job can never be reclaimed by the candidate-building worker. A one-shot activation after this point would be lost forever if the process crashes.

Migration `202823657191_drop_pack_activation_queue.sql` adds `free_pack_activation_jobs`, one immutable row per succeeded R4a PACK job. The PostgreSQL AFTER UPDATE trigger enqueues that row **inside the same transaction** that attaches the verified PACK; a lost COMMIT ACK creates no duplicate. The trigger neither reads S3 nor changes any active pointers.

Activation rows record pending/running/succeeded/failed, a distinct monotonically increasing lease epoch, at most eight attempts, retry eligibility, and terminal failure code. `PgFreePackCatalog::claim_pack_activation` reads **only succeeded** R4a jobs with immutable validated receipts, uses parent-job locking and SKIP LOCKED, renews an exact live lease, and provides bounded retries. Missing schema is an explicit failure; worker remains OFF.

## Acceptance not yet implemented in this slice

- G must reverify every original SINGLE and the full PACK/manifest immediately before promotion.
- B must check **activation lease epoch in the same transaction as the all-member active-pointer CAS**, then atomically mark this activation succeeded. The accepted DROP-05a method only fences the original pack-build epoch; it cannot by itself safely complete this new queue.
- A dedicated activation worker requires an explicit independent `TOR_DROP_PACK_CUTOVER_ENABLED=1` opt-in, and must also require authenticated PostgreSQL, private S3, successful schema probe, enabled R4b3 worker and active PACK reader. Default is OFF. A crash after B CAS but before an ACK must be replay-safe.
- Terminal corrupt source/rights/hold and stale lease must **not** loop forever; record failure and provide operator visibility/metrics before enabling.
- PG18+MinIO injection: candidate commit→enqueue atomicity; concurrent claim; lease expiry/theft; crash before/after G proof; short GET/missing source; B COMMIT ACK loss; CAS all-or-none; pinned V1/age/hold; no fallback, no delete.
- No live deployment, no productive migrations, no shared provider bucket, no historical catalogue migration.

**Do not merge this PR while only the queue is implemented.** Its acceptance gate is a complete fenced B/G worker with exact-head and post-merge CI. DROP-07 remains responsible for physical GC/reconciliation.
