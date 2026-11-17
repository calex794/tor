# DROP-07c — durable, lease-fenced FREE inventory checkpoints

Status: **Draft**, based on accepted DROP-07b / [PR #1024](https://github.com/calex794/tor/pull/1024) (merged as `main@8816663d`). TOR is greenfield and has no existing catalogue. No historic migration, VPS deployment, live schedule or productive SQL application is authorized.

## Why this exists

A full S3 LIST or PostgreSQL receipt scan may take many pages. Restarting the process must not silently lose its position or double count an already committed page. An earlier implementation produced only in-memory opaque pagination tokens and aggregate counts; it was not crash-recoverable.

## Additive PostgreSQL state

Migration `202827531689_drop_free_inventory_cycles.sql` creates `free_storage_inventory_cycles`. It stores one running cycle per **private bucket + direction + namespace**, with:
- strict namespaces `single` / `pack` and directions `pg_to_s3` / `s3_to_pg`;
- monotonically increasing lease epoch, bounded 15–900-second renewals and at most eight claims without a committed page;
- persisted opaque continuation token and page index;
- cumulative aggregate counters (matched, protected, unknown, mismatched and unverifiable);
- status `running/completed/failed`, timestamps and bounded failure code;
- unique partial index that prevents concurrent active cycles in the same scope, even across processes;
- PostgreSQL trigger rejecting DELETE **and any change to bucket, direction or namespace**, so a cursor cannot be reassigned to another private scope.

The PostgreSQL repository `PgFreeInventoryCheckpoints` can claim or recover a cycle, renew an exact unexpired lease, acknowledge exactly one bounded page, or mark an operator-aborted cycle failed. Acknowledgement CAS checks **cycle ID, epoch, expected page index, old cursor and live lease in one transaction** and persists the new cursor and cumulative counts atomically. A lost COMMIT ACK cannot count a page twice; a new claimant resumes the durable cursor after lease expiry. A stale worker cannot renew or append to another worker's cycle. The in-memory cursor advances only after a confirmed PostgreSQL CAS. Committing a nonfinal page releases the lease and resets the retry budget in the same update. The next caller immediately claims the next page at a higher epoch.

`None` as the next cursor completes the cycle. An untrusted caller cannot pass malformed source cursor, invalid direction/namespace or inconsistent counts. Even a status `completed` is **only audit progress**, not a consistent cross-S3/PG snapshot or permission to delete any object.

## Acceptance tests

`crates/books/tests/free_inventory_cycles_pg.rs` uses disposable PG18 to cover new cycle, concurrent claim refusal, exact page accumulation, replay rejection, completion, old epoch expiry/steal, token recovery, immediate handoff across ten pages and exhausted retries without progress, with no S3 write/delete. The test runs in both the mandatory and manual private MinIO-backed workflows with identical commands. Unit tests reject public/invalid cursor and impossible page totals.

## Remaining original DROP-07 gates

1. Wire only a trusted internal/manual operator coordinator to the checkpoint interface, with explicitly configured maximum scan duration, protected S3 credentials and crash-safe heartbeat. There is **no automatic scan at startup**.
2. Repeat a full independent inventory pass to address S3 LIST/PG publication races; verify saved V1, any offline backups, incomplete PUTs, pending imported PACKs and retention under a separate protection index. Checkpoint completion alone cannot prove these.
3. Produce sanitized, compareable, audited dry-run evidence; UNKNOWN remains UNKNOWN regardless of age.
4. Implement quarantine / retention / quorum and an opt-in, security-reviewed GC only in a subsequent explicit PR. No DELETE in this one.
5. Exact-head CI 23/23 followed by complete post-merge acceptance. The accepted #1024 and this independent #1026 must each pass exact-head and post-merge CI.

[Original DROP plan](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
