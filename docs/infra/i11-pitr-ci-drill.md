# I-11 / S061: executable PG18 WAL recovery drill

Status: implementation awaiting exact-head CI. This is a CI-only extension of DR-02 (#744), not a claim of production recovery or authorization to deploy.

The existing `sealed-remote-copy` job already restores a TOR PostgreSQL dump from an encrypted MinIO object and verifies HTTP visibility. Its existing shell entrypoint now also calls `python3 tools/i11_pitr_drill.py`. No new workflow, SSH, deployment secret, production migration, timer or remote database is introduced.

## Physical recovery proof

The new drill builds a PostgreSQL 18 / pgBackRest image and runs it as the unprivileged postgres user. Runtime networking, port publication, host bind mounts, capabilities and inherited application/cloud secrets are absent. Writable state is confined to a bounded temporary filesystem. CPU, RAM, process count and execution time are capped; cleanup addresses only the randomly named test container/image.

Inside the container the drill creates an encrypted physical full backup, commits a second marker AFTER that backup, creates a named PostgreSQL recovery point and then commits a third marker. It confirms all three records exist, switches WAL, checks archiving, stops PostgreSQL and deletes the source data directory. Restoring to the named point must recover the base marker and the post-backup WAL marker, but must exclude the post-target marker. This proves WAL replay rather than merely restoring a base snapshot.

Negative cases: a wrong repository cipher key must be rejected; an unreachable recovery point must fail with the specific PostgreSQL target error; the restored unprivileged role must still be denied table access. Hot standby is disabled in the restored test clusters so startup cannot be mistaken for successful target recovery. No backup, encryption key or database log is published.

## Evidence and limitations

The JSON report includes the source SHA, built image digest, versions, backup label, individual checks and measured drill duration. All checks must be Boolean true; missing/extra fields and substituted truthy values fail. The report always retains `production_tested=false`, `offsite_tested=false`, `application_acl_tested=false`, `rpo_slo_proven=false`, `rto_slo_proven=false`, `release_approved=false`.

The physical test uses a local encrypted pgBackRest repository inside one disposable container. It does NOT prove remote S3 availability, multi-machine disaster recovery, full TOR business-data PITR, paid-object/key restoration, a 15-minute RPO or a four-hour RTO. Its measured duration includes negative tests and is not a production recovery SLO.

The base image remains the repository's existing `postgres:18` family; the built image digest and installed versions identify the actual run. Package repositories are accessed only during image build; runtime network is disabled.

## Acceptance

Run the architectural contract tests, then the full existing Integration CI on the exact integrated commit. Require the `I11_PITR_ISOLATED_DRILL_OK` marker and validated JSON in `sealed-remote-copy`; green compilation or an earlier SHA is insufficient. The helper refuses non-CI execution. No local Docker/PG execution is claimed until an actual run is available.

Remaining line-I gates remain explicit: independent permitted staging; offsite PG/WAL plus objects and recovery keys; distinct-version release/rollback with compatible migrations; HTTP-to-background trace correlation; final REL evidence on one SHA. These are not closed by this drill.
