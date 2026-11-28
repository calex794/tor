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

## RECOVERY-03b: exact UTC cutoff and paused recovery

The existing isolated container also exercises an exclusive time target on
a pinned timeline, using two commits separated by a real UTC cutoff. After
restoring the exact backup set, it starts with fresh socket-only config
instead of inherited auto.conf, checks actual paused recovery via SQL,
verifies both transaction boundaries and shuts down without promoting.
Three additional mandatory Boolean checks cover replay, transaction boundary
and no promotion. These are new acceptance requirements, not yet evidence
of a passed run or external restore.

## RECOVERY-09b: generated config replay in the existing disposable fixture

The UTC replay case now uses the actual RECOVERY-07 file-tree/receipt
checks, RECOVERY-08 configuration generator and RECOVERY-09a verification
helpers instead of maintaining a handwritten startup configuration.
The container build receives only the fixed allowlisted source modules.
It retains its original network-free, unprivileged, bounded runtime and
the original named-target, wrong-key and unreachable-target controls.
The adapter accepts only the random fixture workspace after source
removal. It creates a synthetic local POSIX repository receipt; it is
not an operator entrypoint and cannot establish independent S3 custody,
root-owned operational layout or real host isolation. The actual
operator host gate remains unchanged.

After staging the selected physical PG18 backup, prepare and verify the
configuration under the same local lock, preserve the original auto.conf,
require an empty active auto.conf, and start with explicit config_file.
SQL must confirm the actual config_file, data_directory and port as well
as the existing exclusive UTC/timeline pause and before/after marker
witnesses. The process shuts down without promotion. Schema 2 requires
the additional Boolean generated_startup_config_verified_and_used;
old reports are not silently accepted as proof of this new behavior.
The real test and its marker must pass on the exact accepted CI head.
No operational recovery, SLO or release claim is added.

## RECOVERY-09c: real paused-replay helper execution

After the existing UTC transaction-boundary witness and clean shutdown,
retain the completed fixture and restore the same full backup into a
fresh time-target workspace. The separate CI adapter uses the real
RECOVERY-08/09a preparation and verification helpers and runs
`tor_recovery_pg_run.run_paused` under the existing stage lock.
The container is already the unprivileged postgres OS account: the
adapter removes only the exact runuser prefix from the three allowlisted
generated commands. It does not mock PostgreSQL processes, SQL results,
wait/timeout logic, shutdown or receipt generation, nor invoke or relax
the operator entrypoint or its independent host checks.

The real test requires the private paused-replay receipt with matching
stage/config receipt digests, two stable SQL witnesses, confirmed
shutdown and preserved recovery signal. The receipt must still deny
promotion, exact transaction-boundary proof and operational acceptance.
The independent first UTC case retains its actual before/after witness.
Report schema 3 additionally requires
`operator_paused_replay_helpers_verified=true`; schema 2 is not proof of
this new lifecycle test. The external host, S3, package authenticity,
network isolation and disaster-recovery SLO remain separate gates.

## RECOVERY-10b: real TOR checkout rows through physical WAL/PITR

The sealed logical recovery test has already executed the real D-08
checkout and verified the exact restored encrypted archive. Stream only
that disposable custom-format dump into the existing network-free,
mount-free container at runtime, never into its image or artifact
storage. The launcher requires an exact private CI-owned fixture path,
regular single-link file and a 64 MiB bound; the container independently
bounds and checks the input. No live or operator backup is accepted.

The source physical cluster has an empty tor_ci database and the
two expected disposable roles at base-backup time. After the earlier
named target, import the actual migrated TOR schema and synthetic
checkout rows in a single committed pg_restore transaction. Run the
existing read-only financial witness as tor_app. After the exclusive UTC
target, commit a separate synthetic +100 minor-unit wallet/journal
adjustment and prove that the source balance is now 850. Destroy source
PGDATA. The earlier named recovery must still lack the wallet table;
the paused UTC recovery must yield the exact earlier snapshot (750
remaining, 250 purchase, 50 platform, 200 creator) and no later credit.
Retain the prior operator-helper lifecycle and negative controls.
Schema 4 requires both tor_checkout_finance_replayed_from_wal and
post_target_wallet_change_excluded.

This demonstrates WAL replay of actual TOR financial rows and schema,
not that a second real Rust checkout or funding-provider operation ran
inside the physical source. The import and later adjustment are
synthetic fixture operations. HTTP delivery from that physically
restored database, refunds, Layer and other finance paths, real offsite
custody and RPO/RTO remain separate acceptance gates.

The host also computes SHA-256 over the exact original pre-backup synthetic
finance snapshot, passes only that digest (not the row contents) to the
container, and requires it to match both the post-import and paused-replay
snapshots. Schema 4 additionally requires original_finance_snapshot_digest_matched.
This connects the physical WAL witness to the original actual checkout rather
than accepting an internally consistent but substituted fixture. The digest is
not a signature and the host/CI trust boundary is unchanged. No production
archive, credentials or raw financial rows may be printed or uploaded.

First combined real replay attempt exposed SQL_ASCII inherited from LC_ALL=C
when initdb did not receive an explicit encoding. Initialize the disposable
cluster with --locale=C --encoding=UTF8; require actual Unicode normalization
of decomposed e + combining accent to composed e before the base backup and
after paused WAL replay. The locale controls deterministic fixture ordering;
the change does not alter production database collation or runtime migrations.
Retain every existing financial witness and recovery gate.

## RECOVERY-10c: approved refund and revoked paid entitlement through WAL

Extend the same original finance snapshot and digest with a distinct real D-06
checkout and approved refund. Existing Rust test proves approval idempotency,
300 minor-unit refund, 60 platform and 240 creator reversal, old paid right
revoked, and no duplicate credit. The read-only snapshot checks original and
reversed postings, exact wallet journal, approved decision, revocation and
actual tor_purchase_version_readable=false in the original source, restored
sealed dump, post-import source and paused physical restore. Synthetic wallet
starts and ends at 1000; this is not evidence of a funding-provider journal.
Schema 5 requires tor_refund_finance_and_revocation_replayed_from_wal; schema 4
cannot attest this additional scenario. No new CI workflow or financial rules.
The check is database-level, not actual HTTP/WebCrypto on the physical copy.

## RECOVERY-10d: historical reading and Layer database evidence

Reuse the existing I-11 paid V1/V2/position fixture and the DROP-07 real saved
FREE and Layer checkout/approved ordinary deletion fixture. Include their
read-only database witnesses in the same original snapshot digest covering the
sealed logical restore and physical WAL replay. Verify actual paid entitlement
function allows the pinned V1, refuses V2 and an unrelated account; preserve
reader position, saved FREE identity, archive/deletion records, exact active
PACK generation/receipts, historical Layer membership/terms/debit/command and
revoked term. Do not copy or replace the full Rust FREE/Layer entitlement
policy: the additional witness verifies the selected database facts. The
existing logical restore separately exercises real FREE/Layer HTTP and PACK
bytes. This is not direct HTTP/WebCrypto proof on the physically restored
cluster. The paid fixture uses existing direct synthetic purchase facts;
no real funding or additional payment claim. Schema 6 requires
 tor_historical_paid_free_layer_rows_replayed_from_wal; schema 5 is historical
finance/refund evidence only. No credentials/session hashes in the snapshots.

## RECOVERY-10e-a: actual FREE/PACK and Layer HTTP on a physical copy

Keep the first two UTC paused-review restores and all existing schema-6 evidence.
A third disposable restore of the same exclusive target is promoted only for
application testing. The later wallet transaction must remain absent and the
original finance/rights digest must match before and after HTTP. Enable LOGIN
only for the restricted restored tor_app role on this disposable copy.

Re-download and verify the exact archived PACK and manifest against the retained
inventory and restored SQL receipts. Stream only the bounded synthetic dump,
PACK/manifest and test sessions at runtime; the build context contains only
allowlisted source and Linux executables (the pinned checksum-verified MinIO
and exact Cargo test target). The read-only container retains network=none,
no host mounts, no published ports, no production environment. PostgreSQL,
MinIO and the real Axum FREE/PACK router communicate only on internal loopback.
The shared Rust test now exercises actual TCP HTTP for both logical and physical
restore: saved FREE and historical Layer return exact V1 bytes/hash/no-store;
anonymous, unrelated, revoked and inappropriate readers must get the same
explicit denials. No fallback to database markdown is possible for this fixture.

Schema 7 requires physical_http_primary_cutoff_verified and
physical_free_layer_pack_http_verified. It does not assert full production ACL,
paid V1 HTTP/WebCrypto, real offsite recovery, RPO/RTO or release approval.
Protected paid object restore/decryption on the physical copy remains 10e-b.

## RECOVERY-10e-b1: shared S334 TCP grant transport

Prepare the existing paid V1/V2 S334 recovery acceptance for physical PITR by
replacing its in-process grant requests with actual loopback TCP HTTP. Each
real paid router has its own ephemeral listener, matching origin, bounded
non-redirecting HTTP client and scoped server lifetime. Preserve the existing
logical restore, ciphertext, key, revocation and WebCrypto assertions. This
change does not yet join S334 to the physical PITR fixture; schema 7 and the
independent-host acceptance boundary are unchanged.

## RECOVERY-10e-c1: shared sealed archive and exact S334 WAL snapshot

Seed the actual S334 paid V1/V2 checkouts, with independent KEKs, in the
same disposable `tor_ci` as the existing sealed recovery fixture. Use a
separate loopback source MinIO; copy only verified ciphertext to the existing
backup vault, deny anonymous access, delete both source objects, and retrieve
the exact archived bytes against the original PostgreSQL registry SHA-256.
Keep both KEKs and the synthetic session fixture only in the private CI work
folder. Never transfer either key or session file to the vault or image.

The existing read-only finance snapshot now also runs the independent S334
witness. Compare the composite exact snapshot before backup, after sealed
logical restore, after physical import, during paused WAL replay, and before
and after FREE/Layer HTTP on the promoted copy. Require the original digest
and both new exact CI log markers. This does not yet execute PAID HTTP,
WebCrypto or paid revocation controls inside the physical sandbox. Report
schema remains 7; schema 8 is reserved for that complete integration. No
production keys, production changes, offsite/SLO claim or deployment.
