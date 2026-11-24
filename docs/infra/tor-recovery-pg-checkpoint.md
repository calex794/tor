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
