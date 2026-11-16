# DROP-R4b3 — explicitly enabled, lease-fenced PACK job worker

**Status:** candidate-only background runner. Disabled by default; no S349 route change, no paid ciphertext repacking, no immediate migration/deployment.

## Sequence and authority

1. Existing B-approved job has a frozen physical descriptor and exact hash/frames (`free_pack_job_specs`). A trusted caller must schedule it first; this stage does not expose a browser upload route or select arbitrary editions.
2. `DropPackWorker` claims one runnable pending/expired job via the existing `FOR UPDATE SKIP LOCKED` and monotonic epoch. Delayed pending jobs become eligible after the backoff.
3. B's `approved_pack_sources` rechecks the live job, actor and book ownership, admin hold, each exact FREE `(chapter, version, SHA256, size)` and original S349 SINGLE key/bucket in PostgreSQL. The G reader never trusts job JSON as the source locator.
4. G fetches every original private SINGLE with complete actual bytes and SHA/UTF-8 verification. CPU-heavy bounded Zstd packing is offloaded to a `spawn_blocking` task; dropping the waiter cannot perform S3 or PG effects.
5. Compare built PACK/manifest to B's preapproved descriptor. Check the current epoch immediately before starting S3 writes. A separate heartbeat (every one-third lease duration) maintains the lease while sources, CPU and remote objects are processed. If lease is stolen, expired, cancelled or its renewal fails, execution stops without committing anything.
6. G uses conditional immutable private S3 PUTs and independent R3b full GET of PACK+manifest and all chapters. Only the same verified approved receipt is passed to the existing R4a one-transaction `finish_pack_job`. It fences against post-heartbeat races, author holds, stale epochs and absent source V1. Old workers cannot replace the winner, activate a reader, or delete old SINGLE.
7. Retryable failures settle only the currently live job and epoch into `pending` with 30s backoff, max eight epochs. Terminal source/format mismatches mark only the current job failed. A stolen epoch cannot settle either. A lost PostgreSQL commit ACK leaves a committed succeeded job unaffected by settle; subsequent B idempotent replay can recover original pack ID.

## Operator gates

`TOR_DROP_PACK_WORKER_ENABLED=1` is a **separate explicit opt-in**. It is rejected for demo, unauthenticated server, absent PostgreSQL, builds without `s3-free`, or missing private `TOR_FREE_S3_BUCKET`. The bootstrap probes R4a/R4b1 tables and loads the private G adapter after authenticated startup, then starts one worker per process with a five-second poll interval. No automatic backfill or bulk migration is performed. The default is disabled. Only preapproved jobs are eligible; provisioning those jobs remains a separate trusted scheduler stage.

## Acceptance

- Existing R4b1/R4b2 PG18+MinIO proof, replay and source integrity tests remain.
- New full PG18+MinIO acceptance: publish real SINGLE V1, preapprove exact pack, pause old worker during source I/O, expire lease, let winner reclaim and attach; old process cannot requeue or attach. Original SINGLE and reader active pointer remain unchanged.
- Retry with 30s cooldown cannot be immediately reclaimed; epoch increments on retry, stale worker cannot transition it.
- Bootstrap fail-closed pure tests cover all required environment gates.
- Exact-head Integration CI + Repository rules and post-merge CI are required before declaring accepted.

**Remaining:** trusted automatic selection and enqueueing of eligible FREE editions, operational dashboards, fault injection for real PG outage and provider timeouts, R5 C-authorized Range Gateway, later feature-flagged CAS cutover. Avoid activation on VPS or production migrations until all boundaries are accepted.

[Architecture](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
