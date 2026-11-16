# DROP-R4c — guarded automatic candidate PACK admission

**Status: Draft, opt-in disabled by default.** Builds on accepted R4b1–R4b3. This stage does not activate PACK readers or delete original S349 SINGLE objects.

## The admission invariant

- Exactly **one** `pending` or `running` PACK job is permitted for a book, enforced by PostgreSQL partial unique index. Two planners can do duplicate S3 reads, but cannot commit two different concurrent candidates.
- `free_pack_job_members` stores immutable approved edition IDs transactionally with each B job. Failed or cancelled members are **not silently retried by an automatic scheduler**; an operator must examine them. Successful PACK generations are independently excluded.
- `schedule_pack_job` still validates book ownership, account state, moderation/hold, exact free chapter/version/SHA/size, the full placement receipt, source budget and global fingerprint. Conflicts from the partial unique index return `AuthorError::Conflict`, not a spurious retryable PG outage.
- The planner never trusts frontend-supplied IDs or S3 manifests: B lists eligible FREE S349 immutable receipts, skipping any PACK generation and every prior job member. Older saved FREE V1 source versions are included if still eligible and not already packed; they remain immutable.

## Bounded selection and G proof

- Book IDs are traversed by **keyset pagination of 32**, not OFFSET or any 60-million-row whole-catalog grouping.
- Production policy: at least **16** FREE editions, at most **64**, with a sum of at most **8 MiB** decoded per PACK. Incomplete tails wait until more eligible editions arrive; no tiny one-chapter PACK churn.
- Every batch contains one book and one private bucket. G reads and checks exact original SINGLE bytes, including SHA and UTF-8, then builds a standards-compatible seekable Zstd PACK in bounded off-Tokio CPU work, **before any new job is admitted**.
- Target key is deterministic SHA256 of the book ID and ordered exact immutable chapter/version/SHA/size inventory, within `private/free-packs/auto/{book}/...`. Replanning the identical source set cannot silently switch to a different object.
- B rechecks the current book state and all exact source metadata under its own transaction before committing the job and immutable membership. The separate, previously accepted R4b3 executor does conditional S3 PUT, full independent readback, lease-fenced B candidate attachment.

## Operator controls

- `TOR_DROP_PACK_WORKER_ENABLED=1` activates the preapproved job consumer.
- `TOR_DROP_PACK_PLANNER_ENABLED=1` is **additional**; it is rejected unless the former is enabled with authenticated PostgreSQL and private S3. Both default to disabled.
- Planner startup probes the additional table; missing/unapplied migrations prevent enabling it, not core HTTP startup.
- No public endpoint, no direct PACK URL, no active storage pointer change, no access grants, no paid ciphertext, no GC. Any production backfill or schema rollout is an independent reviewed stage.

## Evidence gate

Disposable PG18+MinIO acceptance tests must demonstrate two concurrent admission calls for one book yield at most one active durable job and matching membership; another distinct batch is denied until completion. The existing R4b3 worker must then commit exactly one verified candidate and retain every original SINGLE and inactive reader pointer. Legacy R4b1 error tests must terminally settle before starting a distinct same-book job.

**Remaining:** post-merge CI, operational queue metrics (eligible/queued/failed), the C-authorized R5 Range Gateway with age/holds/saved-V1 negative checks, and feature-gated CAS switching with rollback validation.

[Notion DROP](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
