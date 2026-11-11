# I-11 / S061: executable PG18 WAL recovery drill

Status: real isolated recovery verified; whole-PR acceptance still required. This is a CI-only extension of DR-02 (#744), not a claim of production recovery or authorization to deploy.

The existing `sealed-remote-copy` job already restores a TOR PostgreSQL dump from an encrypted MinIO object and verifies HTTP visibility. Its existing shell entrypoint now also calls `python3 tools/i11_pitr_drill.py`. No new workflow, SSH, deployment secret, production migration, timer or remote database is introduced.

## Physical recovery proof

The drill builds a PostgreSQL 18 / pgBackRest image and runs it as the unprivileged postgres user. Runtime networking, port publication, host bind mounts, capabilities and inherited application/cloud secrets are absent. Writable state is confined to a bounded temporary filesystem. CPU, RAM, process count and execution time are capped; cleanup addresses only the randomly named test container/image.

Inside the container the drill creates an encrypted physical full backup, commits a second marker AFTER that backup, creates a named PostgreSQL recovery point and then commits a third marker. It confirms all three records exist, switches WAL, checks archiving, stops PostgreSQL and deletes the source data directory. Restoring to the named point must recover the base marker and the post-backup WAL marker, but must exclude the post-target marker. This proves WAL replay rather than merely restoring a base snapshot.

Negative cases: a wrong repository cipher key must be rejected; an unreachable recovery point must fail with the specific PostgreSQL target error; the restored unprivileged role must still be denied table access. Wrong-key and correct-key operations use the same backup, destination and target; only the key changes. A generic restore failure without a successful paired positive control is insufficient.

## SQL readiness, not process startup

Disabling hot standby is not sufficient to make `pg_ctl -w` prove completed recovery. PostgreSQL 18 can report postmaster standby startup before accepting SQL connections. The positive check therefore requires a live postmaster AND a successful SQL query returning `pg_is_in_recovery() = false`, within a bounded wait. Connection rejection and a readable standby are retried, not accepted as success.

The unreachable-target check waits for terminal postmaster exit AND the exact configured-target error. An unrelated crash, timeout or still-running server fails the drill. Executable command-double regressions cover both guards; they do not replace the real PostgreSQL exercise. Diagnostic output is bounded and redacted; no encryption key, backup or raw database log is published.

## Evidence and limitations

The real recovery job passed on tested merge `b641af8b0ec447f6bd320439d9a54642ccaace1d` (PR head `7ca6dffbde421041dcbb439b044889dd195a3352` plus main `149e695eefd22465568378c0d15d9ba465bedfac`):
https://github.com/calex794/tor/actions/runs/43039145236/job/130586321386

All eight recovery checks were true with PostgreSQL 18.6 and pgBackRest 2.60.0. The measured 1.432 seconds covers this tiny local fixture, including negative controls, not operational RTO. Recovery also passed in the subsequent full PR attempt on head `b828e31e567e0b752555a3a6ac7513dcb2d66e86`:
https://github.com/calex794/tor/actions/runs/43040822023/job/130592333914

That whole attempt was NOT accepted: browser dependency installation failed, while PostgreSQL runtime and recovery passed. Revalidate the current composite with the independently landed Ubuntu mirror preparation in main; never remove browser coverage or interpret an old green job as final acceptance.

The JSON report includes the source SHA, built image digest, versions, backup label, individual checks and measured drill duration. All checks must be Boolean true; missing/extra fields and substituted truthy values fail. The report always retains `production_tested=false`, `offsite_tested=false`, `application_acl_tested=false`, `rpo_slo_proven=false`, `rto_slo_proven=false`, `release_approved=false`.

The physical test uses a local encrypted pgBackRest repository inside one disposable container. It does NOT prove remote S3 availability, multi-machine disaster recovery, full TOR business-data PITR, paid-object/key restoration, a 15-minute RPO or a four-hour RTO.

The base image remains the repository's existing `postgres:18` family; the built image digest and installed versions identify the actual run. Package repositories are accessed only during image build; runtime network is disabled.

## Acceptance

Run the architectural contract tests, then the full existing Integration CI on the exact integrated commit. Require the `I11_PITR_ISOLATED_DRILL_OK` marker and validated JSON in `sealed-remote-copy`; green compilation or an earlier SHA is insufficient. The helper refuses non-CI execution. Local Docker/PG execution is not claimed.

Remaining line-I gates remain explicit: independent permitted staging; offsite PG/WAL plus objects and recovery keys; distinct-version release/rollback with compatible migrations; HTTP-to-background trace correlation; final REL evidence on one SHA. These are not closed by this drill.
