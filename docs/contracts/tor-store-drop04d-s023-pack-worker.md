# DROP-04d / S023 — bounded queued PACK-direct import

Source: original Notion «Дроп», DROP-04 and DROP-B03/B05/B06/B08.
Builds on accepted DROP-04a/04b/04c and DROP-06a/06b. No deployment,
live migrations, provider credentials or changes to paid delivery.

## Operation

`TOR_CHAPTER_IMPORT_PACK_DIRECT_ENABLED=1` explicitly selects PACK-direct
only for the existing leased `ChapterImportWorker`. Default remains SINGLE.
Startup refuses this setting without `s3-free`, authenticated PostgreSQL,
private import storage, configured FREE publisher, the S023 worker and
`TOR_FREE_PACK_READER_ENABLED=1`. Invalid values fail startup. Interactive
create/edit and synchronous imports retain their SINGLE contract.

Before destination reservation or upload, B locks the exact live job epoch
and persists `free_chapter_import_transports`: approved identity/fingerprint,
count and SINGLE/PACK choice. The row is immutable. Concurrent choices converge;
a flag change only affects new jobs. Legacy SINGLE reservations/checkpoints stay
SINGLE; an existing PACK stage stays PACK. SQL guards reject checkpoints for
the opposite transport. This additive migration is repository-only.

With the reader configured, disabling PACK rollout still retains a client for
previously selected PACK jobs. Without that client the job retries without
publishing SINGLE or deleting its source. Startup probes the transport schema
before enabling the queued worker; it never applies migrations.

The worker verifies the approved private source and its fingerprint using
existing S023 bounds (700,000 UTF-8 bytes and 500 chapters per job). B reserves
immutable IDs; G creates one independent-frame PACK and sidecar, performs
conditional immutable PUT and complete readback; B stores the immutable stage
under the current job epoch. G independently re-proves the full PACK before
B atomically publishes chapters, versions, UTF-16 shapes, active PACK placements
and succeeded job state. Source cleanup happens only after success. No fake
SINGLE or plaintext PostgreSQL body is created.

A crash after upload/stage leaves no reader publication. Reclaim uses a new
lease epoch and the same reserved IDs, destination and bytes. A stale epoch
cannot finalize or delete the winning job's source. A failure never silently
falls back to SINGLE in the same selected PACK operation. After succeeded
commit, a lost response cannot create a second claimed job. Unreferenced staging
objects require later reconciliation; this change does not physically delete
PACKs or retired editions.

The import flag can be disabled to stop new PACK-only imports. Reader support
must remain enabled while any active PACK-only edition is retained. Disabling
the reader is not a rollback for published PACK-only data.

## Acceptance

- Pure startup-policy tests with and without `s3-free`: default OFF, explicit
  reader/worker/storage dependency, invalid values and demo rejection.
- `drop_04d_real_worker_recovers_staged_pack_after_lease_expiry`: real private
  MinIO source + real PostgreSQL queue; crash after durable stage, actual lease
  reclaim, immutable receipt, exactly two publications, no SINGLE/PG plaintext,
  succeeded state and source cleanup; subsequent worker idle.
- `drop_04d_worker_keeps_single_default_and_denies_held_book`: default SINGLE
  and held-book rejection with no visible partial chapters.
- `drop_04d_100k_bounded_worker_import`, explicit
  `TOR_DROP_LOAD_CHAPTERS=100000`: 200 sequential jobs of 500 chapters, each
  below the existing byte limit. Synthetic multilingual bodies, complete G
  readback, final exact chapter/version/publication/active-placement counts,
  zero SINGLEs or PostgreSQL plaintext. Prints elapsed time and progress.
  This is not a real-book compression ratio or real-cloud cost measurement.
- `free_import_transport_pg`: real PostgreSQL concurrent choice, immutable row,
  stale epoch, flag flip, legacy SINGLE checkpoint and SQL mixed-mode guards.
- Worker policy regressions: staged PACK resumes after rollout OFF; pre-upload
  PACK choice refuses missing reader, preserves source and resumes with rollout
  still OFF; a real legacy SINGLE S3 checkpoint remains SINGLE after rollout ON.
- Existing real Chromium PACK editor adds deterministic owner/revision/hold
  changes immediately after an actual MinIO Range read, before B reauthorization.
- Existing reader/SINGLE/paid tests remain mandatory. Both S3 workflows execute
  the new worker profile and 100k exercise in a separate, freshly migrated
  `tor_test` database, so abandoned leases from older profiles cannot
  enter the new worker queue. Exact-head full CI and post-merge
  evidence are required before accepting this slice.

Remaining original work includes comprehensive fault matrices, independent DR,
PACK inventory/GC/compaction, real provider acceptance and final rollout review.
This slice alone does not close the original plan.
