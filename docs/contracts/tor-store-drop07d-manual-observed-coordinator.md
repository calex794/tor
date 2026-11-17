# TOR DROP-07d — manual one-page S3→PG operator scan (read-only)

**Status:** Draft, stacked on [DROP-07c #1026](https://github.com/calex794/tor/pull/1026). The TOR book catalogue is greenfield; there is no historical backfill.

This slice links the accepted DROP-07b read-only S3 LIST/PG classifier to DROP-07c's durable fenced checkpoints. It **does not start a worker, register HTTP routes, or grant GC permission**.

## Trusted, single-page execution

`OperatorObservedScanner::new` accepts only a trusted backend `PgCatalog`, an already configured private S3 client and a validated private bucket. `run_one(namespace, now)` performs exactly one invocation:
1. Claim a `s3_to_pg` checkpoint for that same bucket and `single` or `pack` namespace, with a 120-second lease. If another worker holds it, return `Busy`, not a competing scan.
2. Read **up to 64 S3 LIST entries** in the fixed namespace, then classify them in B against registered SINGLE/PACK/manifest and protected pending import/repack references. Cap the complete provider+PG step at 60 seconds, half the lease duration.
3. Convert to integer-checked aggregate `matched/protected/unknown` counters. Old or timestamp-uncertain unknowns remain `unknown`; there is no orphan/delete candidate field.
4. Atomically acknowledge the exact page and next opaque S3 continuation in the same fenced PG transaction. A lost COMMIT ACK cannot double count; if the epoch or old cursor changed, return `StaleLease`. Release the lease in that commit so the next invocation can proceed immediately. If there is no next token, record `Completed` as audit progress only.

An unavailable provider or timed-out page **does not write an acknowledged cursor**. The old lease expires and a new worker can recover it with a higher epoch. The code deliberately has no background loop and no actor-facing route.

## Test coverage

- Unit: checked sum of registered/pending/unknown, rejection of over-64 page and integer overflow.
- Disposable PostgreSQL 18 + loopback MinIO: 65 actual S3 objects across two internal scan pages, immediate resumption by a fresh coordinator, one completed audit cycle with all unknown counts stored, and the original S3 object still present.
- CI wires exactly the same command in required and manual private PostgreSQL/MinIO workflows; exact-head success and subsequent post-merge success are acceptance gates.

## Remaining gaps

- This PR handles **only G→B observed pages**. The B→G direction still requires namespace-specific expected-object listing before it may use DROP-07c checkpoints. Do not checkpoint the mixed expected list under a single namespace.
- A complete independent second pass, between-cycle drift detection, saved V1 and separate backup reference proof, operator authentication/CLI, rate/cost budgets, and durable sanitised audit reporting remain unimplemented.
- Even two completed passes do **not** authorize DELETE. Retention, quorum, quarantine and separately approved opt-in GC remain outside this scope.

No server deployment, productive migrations, live API, background task or physical S3 mutation in this change.

[Original Notion DROP plan](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
