# TOR / I-11 / S061 — independent offsite disaster-recovery acceptance

Status: **operator runbook, NOT executed, NOT an authorization to deploy**. This document does not claim that TOR has an external backup or that RPO/RTO are met.

Tracking: [#739](https://github.com/calex794/tor/issues/739). Dependencies: [DR-01 logical restic backup](i11-offsite-backup.md), [DR-02 physical pgBackRest/WAL/PITR](i11-pgbackrest-wal-pitr.md), [DR-03 offsite objects and sealed KEKs](i11-s3-keys-offsite.md), [DR-04 read-only archive monitor](i11-wal-archive-health.md), [CI-only PITR proof](i11-pitr-ci-drill.md).

## 1. Scope and non-negotiable safety boundary

The objective is to prove recoverability **after complete loss of the source server**, not merely to pass CI or list available backups. The operator must separately authorize the drill and supply a trusted **independent, empty restore machine** and the already-existing **external** backup repository. There is no GitHub Actions access to production backups, private age identities or production PostgreSQL.

- No operation on the source server, production PostgreSQL/S3, live IAM, DNS, systemd, production timers or migrations during this acceptance. No backup activation is implied.
- The restore destination must be physically/logically separate from the source, use test-only OS identities and a test PostgreSQL 18 cluster, and be blocked from production payment webhooks, mail, queues and outbound write routes to live S3.
- Verify target PGDATA is empty and located on the approved restore host; target PostgreSQL port must not be 5432, and database names must not be the production name `tor`. A pgBackRest `restore` can overwrite the specified target directory. Never paste a restore command without independently verifying its destination first.
- Recover only from the independently protected **offsite** PostgreSQL/WAL and object/key archives. Reading live primary S3 or falling back to a local MinIO/container voids this test.
- Restrict permissions and retain a secure destruction record for any restored paid content. Never upload database data, object names, KEKs, age private identities, credentials or raw debug logs to GitHub/Notion/CI artifacts.

**Hard stop:** If the source and destination share the same deletable account/credentials, if the backup bucket has no verified retention protection, if the target host can write to production, or if the recovery keys have no independently available offline copy, mark `NOT ACCEPTED`; do not substitute a synthetic success.

## 2. Evidence captured before beginning

Record the exact TOR SHA, PostgreSQL 18/pgBackRest versions, approved restore environment, UTC clocks, destination isolation controls, external account/provider and IAM separation, backup-set identifiers, archive retention, and time of the *latest durable source transaction* before the simulated loss. Credentials and identifying data must remain in the operator's encrypted evidence vault, not this document.

Required offsite assets:

1. Compatible PostgreSQL **physical** full or full+diff backup and an unbroken sequence of archived WAL covering the chosen recovery point. A `pg_dump -Fc`/restic snapshot alone cannot prove PITR.
2. Independently stored encrypted TOR S3 objects (private V1 text and illustrations; plus other published assets), with an independently retained **pre-incident inventory manifest**. Preserve the manifest hash and generation timestamp outside the S3 account being tested.
3. An age-encrypted recovery-key bundle containing **all currently needed and historical KEK versions**, plus an offline private age identity on a different trusted machine. Archive the key-bundle digest independently.
4. An isolated TOR build with its exact Cargo features, compatible migrations/schema and disabled external side effects. All required runtime configuration must have a separately stored recovery copy.

**Partial deletion trap:** `ops/backup-s3-offsite.sh` currently rejects an entirely empty source and byte-checks objects that the source **still lists**, using `rclone check --download --one-way`. That does **not** establish that objects deleted from the source *before the copy* are present. Nor does a nonempty bucket or object-count comparison prove complete recovery. The operator must verify against the independently retained pre-incident manifest and the recovered PostgreSQL `storage_objects` registry, including required prefixes and historical paid-version object IDs. An unreadable or missing manifest is **not** a successful complete-inventory check.

## 3. Execution phases — operator only

### A. Verify the recovery-only boundary

- Verify the restore machine has no production credentials and no live application, payment or S3-write route. Record the network/firewall evidence and the positive identity of the separate host.
- Use separately scoped **read-only** credentials for external backup recovery. Test that the working TOR application identity cannot delete/retire recoverable versions in the backup account; verify bucket versioning, Object Lock/retention and the separate IAM principal through provider-side evidence, not an rclone alias name.
- Inspect backup health and time continuity without changing the production instance. Local CI tests or a green DR-04 probe alone do not establish external backup freshness or a recoverable SLO.
- Freeze the selected backup-set identifiers and an independent source inventory. Do not silently switch to a newer backup if one requested restore point is missing.

### B. PostgreSQL 18 physical restore and PITR

- On the isolated target only, use the operator-reviewed `pgBackRest` configuration and verified empty PGDATA to restore the selected full/diff + WAL to a chosen UTC point **T1**; ensure a witness record committed before T1 exists and a second witness record **T2**, committed after T1, does **not** exist.
- Verify PostgreSQL is accepting SQL, `pg_is_in_recovery() = false`, permissions/role denial still hold, SQLx migration history and key TOR tables are consistent, and startup did not merely return before replay finished.
- Include a marker committed **after the base physical backup** and before T1, so success proves actual WAL replay, not only the original base image.
- Test negative controls on a **separate disposable copy**: removed/missing WAL, wrong repository encryption key and unreachable recovery target must fail rather than silently recover to an earlier/incorrect point. Do not tamper with retained external archives for these tests.
- Keep the source DB entirely untouched. Never import production transactions into an internet-exposed test TOR.

### C. S3 objects and offline KEKs

- Fetch from the independent offsite object archive to the approved isolated destination. Verify the complete path/size inventory against the separate pre-incident manifest and database registry. Record missing/mismatched/extraneous objects explicitly; do not count an object as restored solely because its S3 key exists.
- Run the read-only operator script `ops/verify-s3-offsite-restore.sh` on the trusted independent machine with its documented root-only config and `/run` tmpfs. It verifies one encrypted private V1 object SHA-256 against the recovered database record, and the age-decrypted bundle SHA-256 against the independently retained operator digest.
- Test a known wrong age identity or missing sealed bundle **against a disposable copy**: they must reject access. No plaintext KEK should be left on persistent storage.
- This script is a **sample integrity check only**. It does not prove the full inventory, that the TOR application can decrypt and deliver the V1 or that refund/ACL state is intact.

### D. Business-level end-to-end recovery

Start an isolated TOR using the recovered database, restored encrypted objects and independently recovered KEKs; disable billing capture, webhooks, outgoing mail, background side effects and access to live S3.

Verify at least:

- A buyer can read the **purchased immutable V1** even after a V2 publication, using the matching historical key generation.
- An eligible purchased/privileged image remains private and is served only through authorized access.
- A refunded purchase and a blocked/unauthorized account **cannot** get a new paid-content grant; a public reader cannot access private ciphertext or a KEK.
- Book/library identities, SQLx history and snapshot timestamps match the intended recovery point, including relevant user rights.

If only PostgreSQL tables pass but the restored reader cannot fetch/decrypt the purchased chapter, the drill **fails**.

### E. Failures, alerts and timing

On the **isolated test environment**, simulate loss of external S3 connectivity and delayed WAL push, missing WAL, a wrong key and an unavailable encrypted book. Check that the restoration refuses/alarms rather than promoting a partial result. Test archive-queue/disk thresholds without filling or disturbing production disks.

Verify that the **external alert consumer** receives the expected incident. A log line inside the machine that has supposedly failed is insufficient.

Use the same explicit UTC clock source for:

- `T_loss`: time of the simulated total loss.
- `T_latest_source_commit`: latest known committed witness before the loss (or a controlled heartbeat under writes).
- `T_latest_restored_commit`: latest witness actually observable after restoring external backups.
- `T_recovered_service`: time a *verified HTTP paid-content + ACL probe* against isolated TOR succeeds.

Calculate `RPO = T_latest_source_commit - T_latest_restored_commit`, not `archive_timeout` or age of a local file. Calculate `RTO = T_recovered_service - T_loss`, not mere completion of database restore. During an idle interval, **record RPO as unproven** unless a controlled write/heartbeat makes the result measurable. Do not substitute fixture timings for operational RTO.

The design **targets** RPO ≤15 minutes and RTO ≤4 hours, subject to explicit operator approval. A missed target is a failed drill, not a reason to weaken the test.

## 4. Redacted evidence report (template, NOT a passed result)

Store the completed report and any raw/private supporting material in the approved encrypted operator evidence store. The copy attached to #739/Notion must be redacted. Leave unknown values as `null`, not fabricated zeroes.

~~~json
{
  "scope": "independent_offsite_restore",
  "status": "NOT_ACCEPTED",
  "source_sha": null,
  "postgres_version": null,
  "pgbackrest_version": null,
  "restore_host_separate": false,
  "production_untouched": null,
  "backup_set_ref_redacted": null,
  "source_inventory_manifest_sha256": null,
  "database_pitr_t1_present_t2_absent": null,
  "wal_post_base_backup_replay_verified": null,
  "external_s3_and_iam_separation_verified": null,
  "object_lock_and_retention_verified": null,
  "full_s3_inventory_verified": null,
  "private_v1_hash_and_offline_kek_verified": null,
  "purchased_v1_after_v2_verified": null,
  "refund_and_unauthorized_acl_denials_verified": null,
  "negative_controls_failed_safely": null,
  "external_alert_delivery_verified": null,
  "t_loss_utc": null,
  "t_latest_source_commit_utc": null,
  "t_latest_restored_commit_utc": null,
  "t_recovered_service_utc": null,
  "measured_rpo_seconds": null,
  "measured_rto_seconds": null,
  "operator_signoff_ref": null
}
~~~

**Acceptance rule:** Every applicable affirmative control must be supported by *independent* archived evidence; observed negative cases must fail safely; RPO/RTO must be measured from the recorded timestamps; all secrets remain off public systems. A report with missing evidence, merely green CI, a successful local MinIO fixture or a single V1 checksum is **NOT ACCEPTED**. Only after manual review and operator sign-off may #739 and S061 be reassessed. This runbook creates no scheduled jobs, no new live backup, and no automatic approval.

## 5. What is already demonstrated, and what is not

- Existing CI: `tools/i11_pitr_drill.py` proves PG18 physical full + WAL recovery within one isolated local container, including wrong-key/unreachable-target negative checks; `sealed-remote-copy` and `postgres-runtime` exercise disposable environment recovery. Post-merge CI on `main@ca9acb3855c298a5811455c3906c06c85ffc5fea` passed [17/17](https://github.com/calex794/tor/actions/runs/43530755043).
- **Still not demonstrated:** independently protected offsite PostgreSQL and S3 archives, complete inventory after partial deletion, offline custody of *real* recovery keys, full independent-host TOR/ACL recovery, real outage notifications and measured RPO/RTO.
- No change in this document alters the live VPS, primary PostgreSQL, production S3, IAM, source objects, backup timers or migrations.
