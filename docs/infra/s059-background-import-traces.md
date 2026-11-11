# S059: durable correlation from HTTP to chapter-import attempts

Status: implemented in Draft #832, actual exact-head CI acceptance pending. No operational deployment, database migration, service activation or production SLO is approved by this document.

## Causal boundary

The existing server-generated request trace ID is inserted into `chapter_import_jobs` with a new job. The HTTP payload/header cannot select the origin. An idempotent replay reads the existing job and never replaces its origin. PostgreSQL enforces a nullable 16-byte nonzero shape and immutable updates. Existing jobs retain NULL; there is no invented backfill.

Each claimed import attempt receives a distinct server trace ID. Its parent survives worker restarts and lease reclaims in PostgreSQL. Outcomes distinguish completed, retried, failed, stale ownership, interrupted observation and unavailable persistence. The import transaction and fencing epoch remain authoritative for business state. An Interrupted receipt can reflect cancellation after an uncertain commit; it is not proof that a transaction rolled back. SIGKILL can prevent publication entirely and must never be fabricated as a successful or measured attempt.

Four sequential wall-time phases cover verified source acquisition, parse/fingerprint, queue/chapter database work and the cleanup attempt. They are not SQL server execution time or CPU consumption. A completed import does not assert successful cleanup of its staging object. Child duration never alters the completed HTTP root duration or its SQLx evidence.

## Transport and storage

The unchanged HTTP wire is supplemented by a fixed 80-byte numeric child frame. No user, book, job, filename, source text, object key, SQL or caller-supplied trace string is serialized. Encoding validates identity, attempt, phase, outcome, timing bounds and arithmetic. Both frames share the existing bounded nonblocking publisher queue and loss counters; business work never waits for SQLite or IPC.

The independent writer owns a separate indexed SQLite child table. Retention and row limits are bounded; the existing whole-file size ceiling remains. Children can arrive before roots and survive independently of missing/expired roots. Repeated identical receipts are idempotent; conflicting receipts with the same ID are rejected. Exact-parent lookup returns an exact retained count and at most 32 newest receipts, not an assertion about all work ever performed.

The existing `/ops/traces` authority and no-store/security headers remain. Ordinary login cookies do not grant operator access. Four bounded blocking readers hold permits even when their HTTP waiters time out. A missing root does not hide children. A disabled, older or failed archive is explicitly unknown; an empty successful lookup does not prove absence or completion of work.

## Executable acceptance

The existing `reader-browser` S059 step still runs the original real PostgreSQL delay, SQLx, writer crash/restart and operator-browser checks. It additionally invokes `s059_http_import_retry_and_restart_evidence` from the dedicated Rust integration target with explicit disposable database, directory and socket guards. No new workflow or operational secret is introduced.

The fixture uses actual loopback HTTP, real PostgreSQL sessions/jobs, separate import-worker processes, the actual standalone SQLite writer, and Chromium at 1440 and 390 pixels. A private filesystem source-store fixture injects source failure, corruption and a synchronized gate; this is deliberately not an S3-provider test. Existing private-S3 acceptance remains separate and required.

The scenarios cover denied enqueue, client-origin spoofing, immutable replay origin, retry then success, writer restart durability, cancellation/stale fencing, cooperative interruption, hard-killed attempt without invented receipt, lease reclaim in another process, corrupt-source failure and successful business completion during writer outage. HTTP roots are compared before and after worker completion. Catalog latency is measured while a worker is gated; the 2000 ms debug-CI ceiling is not a production SLO.

Successful reports are merged into the existing `s059-pg/report.json`; only that report and screenshots are uploaded. Private source files, SQLite databases and operator credentials stay under the excluded private directory. The report is produced only after actual assertions and browser execution; source checks alone are not runtime proof.

## Remaining limits

This patch instruments chapter import, not every consumer, cover worker, scheduler or cleanup task. It does not prove production p95, independent staging, durable lossless telemetry, offsite recovery or release authorization. `other_background_workers_tested`, `production_slo_proven`, `independent_staging_tested` and `release_approved` stay false. Whole-line I/S059 acceptance must use the reviewed scope and real evidence, not the existence of this code.
