# TOR I-05: original PostgreSQL migrations imported from bokarust

The nine SQL files in this directory are **unchanged Git blobs** copied from
[calex794/bokarust, commit d11b27b43125b93e59809147e8e687203fa0753b](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b/crates/adapters/migrations).
**Do not modify any of the nine historical files.** The \`SOURCES.tsv\` manifests original Git
blob hashes, validated by \`python3 tools/check_migration_sources.py\`.

Migrations have monotonically increasing SQLx filename versions. The historical
\`bokarust\` SQLx adapter and app logic have **not** yet been moved to TOR; this
is the first schema handoff, not a connected product. The GitHub
\`TOR PostgreSQL 18 migration contract\` workflow spins up an ephemeral PostgreSQL 18
and uses \`psql\` to validate **fresh install, upgrade with existing data and core
schema invariants**. The CI smoke applies SQL directly to disposable databases;
it is **not** the production migration runner and does not populate
\`_sqlx_migrations\`.

**Do not run \`psql -f\` blindly against the VPS TOR database.** The next I-line
deliverable must add the canonical SQLx migrator, verify checksums and version
history, and take a database backup. Only after this step may the migrations be
applied to \`tor\` by \`tor_migrator\`, followed by actual SQLx repositories and the
HTTP → PG → browser path. The VPS \`tor-web\` is still RAM-demo.

The app \`tor_app\` has only DML access and lacks schema CREATE. Never use
the PostgreSQL superuser for the running site; passwords remain exclusively
on the VPS in \`/etc/tor/tor-app.env\` and \`/etc/tor/tor-migrator.env\`.

## G-07/G-09 cover extension (2026-10-26)

An additional migration `202610260001_cover_image_jobs.sql` creates G-owned verified upload objects, leased cover jobs, immutable WebP variant pointers, and a **B-owned** book cover slot updated by revision CAS. The nine historical donor blobs and their hashes remain **unchanged**; source checker verifies the original 9 plus this explicit extension. The official `tor-migrate` runner applies the tenth migration as `tor_migrator` (not `tor_app`). Real CI PostgreSQL tests prove actor ownership, exclusive claim, lease fencing, retry, atomic B slot update and old generation retention with a simulated S3 port. Production S3 authorization and the browser upload endpoint require separate deployment evidence.

## RIL-06: PostgreSQL rollups (2026-10-26)

`202610260002_ril_rollups.sql` extends, but does not change, all earlier migrations. It adds immutable keyed batch staging, leased/fenced claims, hourly route/transition aggregates and a daily rollup. Only anonymous fixed route/status/latency codes are stored. A background worker enqueues bounded batches at intervals and applies them atomically; **HTTP never executes this SQL**. Retry of an identical batch key is idempotent. An accepted HTTP event still may be lost before staging on process crash (best-effort analytics); once a batch is committed, crash/lease recovery replays it without double counting. Finished batch tombstones remain for 90 days; hourly summaries for 30 days, daily summaries for 90 days. Protected rights, payments and progress retain their separate durable contracts. CI runs real PG18 lease fencing, duplicate stage, restart, retry, expiration and outage tests through the restricted `tor_app` role.

## S023 chapter import queue (2026-10-31)

`202610310247_chapter_import_jobs.sql` adds the durable owner-scoped queue for already-reviewed canonical UTF-8 chapter text. Only private staging-object metadata and a SHA-256 are stored in PostgreSQL; the text body stays in private S3. One active job per book, idempotency key, bounded attempts, lease epoch and expiry, owner-scoped status, cancellation and terminal outcomes are enforced by schema and repository operations. The final job success and the existing all-or-nothing `import_draft_chapter_batch` writer must commit in the same transaction while holding the job row lock. A cancel that wins first leaves zero chapters; a finalizer that wins first makes cancellation a no-op. The worker is default-off and requires explicit `TOR_CHAPTER_IMPORT_WORKER_ENABLED=1`, authenticated PostgreSQL, and a configured private S3 bucket. The HTTP path stores only the author's reviewed canonical text; DOCX/ZIP extraction remains in the existing preview step. CI covers PostgreSQL lease recovery, owner privacy, cancellation/finalization races, HTTP status, worker completion, and loopback MinIO integrity/private access.
