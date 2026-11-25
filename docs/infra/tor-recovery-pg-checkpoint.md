# TOR Recovery / I-11 / S061 — RECOVERY-03a: stage a pinned PostgreSQL checkpoint

Depends on RECOVERY-02 (#1214). No operation on the working VPS or production
PG/S3. This slice restores physical backup files only; PostgreSQL startup,
WAL replay, application-level validation and DNS cutover are separate gates.
Tracking: [#739](https://github.com/calex794/tor/issues/739).

## Independently choose the recovery point

From the protected operator archive, select the exact pgBackRest backup set
label, source timeline, UTC target and earliest known incident cutoff.
The selected backup must be compatible with the target and all required WAL
must exist in the external archive. Never select latest automatically.
An exclusive target stops before transactions at the target instant.
Timeline selection is explicit to avoid silently following an unwanted branch.
For compromise, a trusted incident cutoff must be established independently;
the script cannot discover when the attacker entered.

## Private recovery-only repository

Use a different approved host prepared by RECOVERY-02 and verify provider
IAM, retention and network boundaries separately. The recovery identity must
be read-only. The working book bucket tore is not a backup repository.
Create a root-owned 0600 JSON file, with a new independently scoped recovery
credential pair, at /srv/tor-recovery/evidence/pgbackrest-repository.json.
Required string fields: endpoint (DNS hostname), bucket, region, repo_path,
access_key, secret_key, cipher_pass (independently recovered pgBackRest
passphrase, at least 32 characters). No arbitrary pgBackRest options, extra
fields or secrets in CLI arguments. Values with unusual characters outside
the supported grammar are refused, not escaped into configuration.
Do not use actual keys in examples, CI or GitHub.

## Offline plan, then explicit staging

```bash
sudo python3 tools/tor_recovery_pg.py \
  --source-machine-id "$SOURCE_MACHINE_ID" \
  --target-machine-id "$TARGET_MACHINE_ID" \
  --backup-label "$APPROVED_BACKUP_LABEL" \
  --target-utc "$CLEAN_TARGET_UTC" --incident-utc "$INCIDENT_UTC" \
  --timeline "$APPROVED_TIMELINE" \
  --endpoint "$BACKUP_ENDPOINT" --bucket "$BACKUP_BUCKET" \
  --repo-path "$BACKUP_REPO_PATH"
```

Without --restore the command validates only the checkpoint syntax and
chronology, returns PLAN_ONLY and accesses no repository. It does not
prove backup existence. With --restore and
--operator-confirm-read-only-repository it checks the approved distinct
machine ID, rejects an existing TOR installation and nonempty/partial
workspace, validates the private repository settings against the separate
operator selection, creates a restricted pgBackRest configuration and
runs pgBackRest as postgres with a clean environment. Restore uses the
explicit set, time, timeline, exclusive target and target-action=shutdown.
No force/delta or source path mapping; all backup tablespaces are remapped
inside the recovery workspace. Credentials remain in the protected local
config, and raw command output is not sent to CI or the terminal.

On error retain the partial workspace for private operator inspection.
Do not automatically delete or rerun into existing PGDATA. A successful
FILES_STAGED marker only checks return code and physical PG18 file markers;
it does not prove reaching the time target or successful WAL replay.

## Next gate: RECOVERY-03b

Review restored postgresql.auto.conf, links, includes, extension/preload
settings, database code and generated recovery settings before any startup.
Use trusted fresh PG18 executable, local-only socket configuration, forced
archive_mode off, verified restore_command and no live upstream connection.
Verify exact recovery target, timeline, post-backup WAL witness and
post-target absence on the isolated cluster. The shutdown action deliberately
does not automatically promote; a separately reviewed startup and promotion
path must be tested. Reconcile restored objects, KEKs and rights before
starting TOR; no external side effects or DNS until independently approved.

References: [pgBackRest restore command](https://pgbackrest.org/command.html),
[independent TOR DR acceptance](i11-independent-dr-acceptance.md).

## RECOVERY-03b internal evidence (same PR, no external start)

The existing mount-free PG18/pgBackRest fixture now additionally restores
its exact full backup set on explicit timeline 1 to a real whole-second UTC
point between two committed transactions. It discards inherited auto.conf
from the fixture startup path, uses a new local-only config, replays WAL,
and requires SQL to prove recovery is actually paused, not just requested.
The earlier witness must exist, the later transaction must be absent and
the replay timestamp must precede the exclusive target. A final check
requires the instance still in recovery; stop without promotion and retain
recovery.signal. The evidence gate requires all three new checks.

This tests the recovery semantics and config approach on synthetic data,
not the safe startup of an arbitrary potentially compromised restored
cluster. The external operational startup tool and trust/extension/link
validation remain a separate gate; never run the CI fixture on a real
server. See [the existing CI drill](i11-pitr-ci-drill.md).

## RECOVERY-07: pre-start file and selection checks (no startup)

A successful fresh RECOVERY-03a stage now writes the private,
non-overwriting `evidence/pg-stage-receipt.json` only after successful
pgBackRest staging and the initial physical file checks. It pins the
operator-selected machine IDs, backup label, UTC cutoff/target, timeline,
port and digest of the generated recovery-only pgBackRest configuration.
It is not a signature, an independent backup inventory or proof of WAL
replay. Do not fabricate a receipt for an older or partial stage.

On the separately approved, quiescent isolated restore host, repeat the
same independently selected CLI arguments from above with
`tools/tor_recovery_pg_prestart.py` (omit `--restore` and
`--operator-confirm-read-only-repository`; add
`--operator-confirm-quiescent-isolated-host`). This reads only local
files and confirms the private stage receipt, exact generated config
digest and empty include directory. It refuses unexpected file
ownership/permissions, symbolic and hard links, special files,
different-device mounts, active/standby markers and incorrect PG18
markers. The supported first slice is deliberately limited to a
single filesystem with no tablespaces, external WAL symlink or group
readable restored data. Unsupported layouts require separate review;
never loosen the checks merely to make a restore pass.

The host must actually be quiescent, the postgres OS account unused by
other processes, and filesystem/network isolation independently verified
(including same-device bind mounts). The tool does not prove those
properties or withstand a concurrent writer. It does not authenticate
arbitrary physical page/WAL/catalog contents, validate extensions or
recover compromised secrets.

`TOR_RECOVERY_PG_PRESTART_FILES_CHECKED` is a read-only intermediate
result, **not permission to start PostgreSQL**. Original configuration
including postgresql.auto.conf remains untouched and untrusted. A
separately reviewed step must neutralize inherited settings, generate
trusted recovery-only configuration, check executable/catalog risks,
run PostgreSQL with external side effects blocked, prove the actual
exclusive paused UTC replay and explicitly approve any later promotion.
No part of this tool starts a process or contacts a repository.

## RECOVERY-08: trusted startup configuration preparation (no startup)

After a fresh stage and RECOVERY-07 preflight, use
`tools/tor_recovery_pg_startup_config.py` with the same independently
selected checkpoint and host arguments, including the quiescent isolated
host confirmation. Supply all six explicit resource settings:
`--max-connections`, `--max-prepared-transactions`,
`--max-locks-per-transaction`, `--max-wal-senders`,
`--max-worker-processes`, `--shared-buffers-mb`. Choose these from
independently trusted source settings and target capacity. PostgreSQL
18 hot standby requires the five corresponding limits to be at least
the source values across replay; insufficient settings can interrupt or
pause recovery before the target. See
[PostgreSQL 18 hot standby](https://www.postgresql.org/docs/18/hot-standby.html).
The tool checks bounded syntax, not the actual source high-water marks
or available memory. Unsupported values require separate review.

Default is a local-only plan. An explicit `--prepare-config` creates
a private, non-overwriting attempt record under the existing stage lock,
after repeating preflight under that lock. It generates a new
root-owned recovery-only config, HBA and ident file under
`config/pg-startup/`. They specify the selected exclusive UTC/timeline,
pause (not promotion), local peer access only for postgres, no TCP
listener or upstream, no archive push, empty preload and recovery hooks,
and the exact pinned recovery-only pgBackRest archive-get command.

Preserve the inherited postgresql.auto.conf byte-for-byte as private
`evidence/pg-original-auto.conf` before replacing that one file with an
empty postgres-owned regular file. The original may contain secrets;
do not display, export, or commit it. All other inherited configs remain
untrusted. Never start using the default PGDATA config selection; a
future separately reviewed launcher must force the new config_file and
verify the completed `evidence/pg-config-receipt.json` against the
approved selection, original stage receipt and generated bytes.
The receipt records digests only, and startup_authorized=false.
A partial attempt is retained and must not be retried automatically.

`TOR_RECOVERY_PG_CONFIG_FILES_PREPARED` proves only local preparation.
It is not a filesystem integrity signature, OS isolation proof, safe
catalog/extension validation, or permission to start the cluster.
Source credentials, roles, functions, extensions, catalog settings and
WAL/data bytes remain untrusted. Independent network/OS isolation,
trusted executable review, actual paused WAL replay with SQL witnesses,
and separately authorized promotion remain separate gates. Never run
this operator path in GitHub Actions or on the working server.

## RECOVERY-09a: verify completed configuration before startup

After successful RECOVERY-08 preparation, run
`tools/tor_recovery_pg_config_verify.py` with the same independently
selected host/checkpoint and six resource settings (without
`--prepare-config`). It repeats the host and RECOVERY-07 preflight under
the existing nonblocking stage lock. It then independently renders the
expected PostgreSQL config, HBA and ident bytes and requires the exact
three-file inventory, safe ownership, modes and contents. It checks the
attempt marker, private preserved original auto.conf, empty active
auto.conf and absence of a partial replacement. Recompute the complete
canonical config receipt from the verified selection and files; do not
accept an edited config merely because its digest in a receipt matches.

`TOR_RECOVERY_PG_CONFIG_VERIFIED` is read-only evidence, not authorization
to start. The receipt is not an independent signature. A concurrently
modified PGDATA, same-device bind mount, compromised operating system,
catalog, extension or executable remains outside this verifier's trust
boundary. The future launcher must repeat this verification under the
same lock immediately before any startup and independently enforce the
isolated environment. Next: real replay using generated config in a
disposable fixture, then the separately guarded operational launch and
paused-state/transaction-boundary witness. No automatic promotion.

## RECOVERY-09c: separately authorized paused replay, then shutdown

After the generated-config real fixture succeeds and RECOVERY-09a is
accepted, use `tools/tor_recovery_pg_run.py` with the same independently
selected host/checkpoint and six resource arguments. The default checks
only the prepared files and launch-artifact absence. No process starts.

Actual execution requires both `--run-paused-replay` and
`--operator-confirm-reviewed-isolated-execution`, in addition to the
existing independent/quiescent host confirmation. **Do not confirm**
until a separately reviewed disposable VM/host has trusted OS/packages
and loader/libraries; exclusive postgres identity and no other workloads;
restricted egress only to the independently pinned read-only pgBackRest
repository; no production credentials, writable external mounts, or
network access to the source/primary; and an independent review of
untrusted restored catalogs, extensions and WAL. The CLI checks root
ownership, non-writable ancestry and exact executable paths, not
cryptographic package authenticity or firewall policy. These operator
and infrastructure requirements are not satisfied by a flag alone.

Choose an explicit bounded `--replay-timeout-seconds` (60–14400,
default 300) appropriate to the selected backup and WAL volume.
Under the existing stage lock, rerun host/preflight and exact config
verification before creating a durable, non-overwriting launch attempt.
Invoke only fixed installed PG18 tools as the unprivileged postgres
account with cleared environment, explicit config_file, private log
and socket. A successful pg_ctl start is not replay success. Query only
built-in catalog state, require the intended effective recovery settings,
two equal actual paused WAL LSN/time witnesses and a last replayed
transaction strictly before the chosen UTC target. No SQL input from
the operator or business tables is executed in this step.

Always attempt a bounded fast shutdown, including after failed startup,
failed witness or handled interruption; require disappearance of the
postmaster PID and empty socket directory plus preserved recovery.signal.
On uncertain shutdown, keep the independent host isolated and inspect
it manually. A process or VM killed outside the tool may still require
manual containment. Never clear the launch attempt or retry automatically.

The private `evidence/pg-paused-replay-receipt.json` is written only
after successful paused-state witness and confirmed shutdown.
It deliberately retains exact_target_transaction_boundary_verified=false,
promotion_authorized=false and recovery_accepted=false. Pausing may
also occur for other reasons; a stable LSN/time before the target does
not establish the exact transaction/business boundary. Independently
review recovery logs and actual before/after transaction and TOR
purchase/ACL/key/object witnesses in later steps. Never resume replay,
promote, start TOR or deploy automatically. Only a disposable real PG18
test and separately approved external exercise can establish the
remaining evidence. See PostgreSQL 18 documentation:
https://www.postgresql.org/docs/18/runtime-config-wal.html
and https://www.postgresql.org/docs/18/functions-admin.html.
