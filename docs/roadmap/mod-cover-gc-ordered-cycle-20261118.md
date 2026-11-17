# MOD-COVER-OPS-01 / #667 — one bounded private-cover cleanup pass

**Status: code composition and isolated tests only. No automatic private GC is enabled.** Real S3/CDN, VPS and persistent DB are outside the scope.

## Scope

`crates/web/src/cover_private_gc_ops.rs` composes the already-merged B-owned deletion services. The only method `run_single_bounded_batch()` makes one bounded pass, without a timer, loop, startup hook, HTTP route or any operator command:

1. Stage and lease rejected/superseded **private WebP variants** using the existing `PgPrivateCoverVariantRemovals` safeguards and exact staging variant keys.
2. If any variant deletion failed or was deferred, **skip the entire shared-source phase** for this cycle. A separate future pass may retry based on B's durable lease and backoff.
3. Otherwise, stage and lease **private shared source originals** only when every referencing job has a terminal review, every variant is marked removed and no current B Book slot references the source.
4. Return counts of claimed/removed/deferred variants and optionally source counts. No original content, keys, credentials or reviewer identities in the result.

The underlying services enforce 12-second S3 timeouts and 60-second fenced PG leases, bounded batches (variants up to 3 claims, sources up to 2) and crash-safe retries.

## Verification

The existing real PG18/MinIO cover test now invokes the coordinator after aging a rejected recipe while another source recipe remains pending. It requires that the rejected variant pair is removed, **zero originals are claimed**, and the original object still exists. The existing separate two-worker PG lease race, historical audit, source replay-denial and active approved slot tests remain intact.

`tests/architecture/test_mod_cover_gc_manual_boundary.py` also guards against accidentally starting this worker from the web runtime or introducing HTTP routes/scheduling.

## Deliberately not enabled

There is **no CLI** to call this function and **no environment flag** that activates it. A future operator-only executable requires its own authorization contract, dedicated private-cover S3 credentials, no public CDN or paid-object permissions, explicit retention policy and legal/appeal holds, metrics, safe-stop behavior, and a reviewed dry-run from #1051.

This module is not a release or deletion authorization. #667 stays open until real operator dry-run, orphan evidence and policy signoff. CDN edge revocation remains #684.
