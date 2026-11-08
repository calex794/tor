# I-11 / DR-01 — PostgreSQL 18: encrypted off-site snapshots

[Tracking issue #739](https://github.com/calex794/tor/issues/739) · [TOR-HOME-01](https://app.notion.com/p/40832a84beec8103b8d3c43170472381)

Status: **operator tooling only; NOT installed, scheduled or validated on live data**.
This is the initial recovery baseline, **not** point-in-time recovery. It must not
be used as evidence of RPO 15 minutes or of a production launch gate.

## What exists already

The project has an isolated CI test using PostgreSQL 18 and disposable MinIO:
`tools/tor_dr_s3_backup_smoke.sh`. That test proves application/ACL recovery
with a small throwaway archive; its encryption helper has a **16 MiB CI-only
limit** and cannot be used for a real database. This new operator path uses
`restic` streaming encryption, and does not reuse the CI helper.

## Separation and prerequisites

- Root access on the *actual* PostgreSQL host; all secret files must be
  `root:root` and mode `0600`. The backup uses only the local socket;
  PostgreSQL must not be exposed publicly.
- PostgreSQL 18 binary package, `restic` **0.17 or newer** supporting
  `--stdin-from-command`, `runuser`, `flock`, `python3`.
- A **dedicated encrypted restic S3 repository**, separately initialized
  manually by the operator (`restic init`), in a backup bucket independent
  of TOR's live book/media bucket. Do not use the backup identity to modify
  live media; protect archives with bucket versioning/retention and credentials
  separated from the application.
- A secret-only file `/etc/tor/offsite-backup.env` (owned by root, mode
  600), containing these **variable names** (never put real values in Git):
  - `RESTIC_REPOSITORY=s3:https://<backup-endpoint>/<backup-bucket>/<prefix>`
  - `RESTIC_PASSWORD_FILE=/etc/tor/offsite-restic-password`
  - `AWS_ACCESS_KEY_ID=<backup-only-key-id>`
  - `AWS_SECRET_ACCESS_KEY=<backup-only-secret>`
  - Optionally S3 region/provider-specific restic variables documented by
    your provider. The backup password is a separate root-only 0600 file.
- Offline recovery copies of the restic password and the S3 access-recovery
  procedure (not the running host's only copy). A password loss means loss
  of backup access.
- The PostgreSQL `postgres` OS user must authenticate on
  `/var/run/postgresql`; the actual cluster/database must be `18/main` /
  `tor`. If TOR moves to a managed remote PostgreSQL instance, use a
  separately designed backup strategy: this script intentionally refuses it.

## First use (operator review mandatory)

1. Review the script on the exact approved commit, make sure the remote bucket
   is private, credentials are backup-scoped, secret files permissions match,
   and the S3 account **cannot delete all recoverable history**.
2. Install the pinned script as
   `/usr/local/sbin/tor-offsite-backup` root-owned mode 0750.
3. Test once manually: `sudo /usr/local/sbin/tor-offsite-backup`. Check
   `TOR_OFFSITE_BACKUP_OK` exit 0 and verify the new timestamp using
   `restic snapshots --tag tor-pg18` from an independent trusted machine.
4. On a **separate sandbox PG18 on port 15432–15999**, pre-create an empty
   database whose name starts with `tor_dr_`, with a restore-capable role.
   Never use production role/endpoint. Prepare an operator-only environment
   variable `TOR_DR_RESTORE_URL` in the form\n   `postgresql://tor_restore@127.0.0.1:15432/tor_dr_test` (no password or\n   URL query parameters), plus a separate root-only 0600 `PGPASSFILE`\n   configured for the test role. Never pass a password in the URL or Git.
   Install `verify-pg18-offsite-restore.sh` as
   `/usr/local/sbin/tor-offsite-restore-verify`, review/execute it.
   It reads the latest tagged snapshot and restores into the already empty
   database, checks migration history and books table; **it does not delete
   the sandbox afterward**. The operator must remove it safely.
   This is NOT proof that the snapshot is externally independent if both
   repository and source are on the same provider/account.
5. Only when the manual backup and restore both pass, install the provided
   `tor-offsite-backup.service` and `tor-offsite-backup.timer` templates into
   systemd. The timer is not enabled by PR/CI. Enable explicitly after
   approval: `systemctl enable --now tor-offsite-backup.timer`.
   Observe errors in `journalctl -u tor-offsite-backup.service`; configure
   off-host alerting for failed/late jobs.
6. Rotate/revoke S3 credentials without deleting prior archives. Re-check
   restore after every relevant credential or server change.

## Controls

- Dumps are streamed directly to restic and encrypted before off-site upload;
  **no local plaintext dump file** is written by the operator script.
- The scripts refuse missing/unsafe config and repository URLs not starting
  with HTTPS S3; no repository auto-initialization or deletion is performed.
- The backup is a logical PostgreSQL custom-format dump. It does not include
  server-wide users/tablespaces, `pg_hba.conf`, TLS files, TOR service config,
  S3 objects, secrets or purchased chapter encryption keys. Preserve those
  separately. A full TOR recovery requires verifying those pieces together.
- Never restore a production backup within CI or upload database archives to
  GitHub Actions artifacts. The existing disposable CI tests stay separate.
- `pg_dump` at one consistent snapshot does not automatically reproduce every
  payment/webhook event that a remote provider recorded later. Reconcile
  with the payment provider before resuming real payments.

## Next slices

1. Implement **pgBackRest + WAL/PITR**, monitor archive age/retention, test
   point-in-time recovery. Only then measure RPO with the target of 15 min.
2. Independently back up S3 objects and the **separate encryption keys**.
   Restore a purchased immutable V1, permissions, refunds and denied book.
3. Automate periodic recovery tests on a separate trusted machine and alert
   on failed or stale snapshots; measure RTO and file proofs.
4. Consider a warm PostgreSQL replica only if actual availability goals
   require it. Replication is not a substitute for durable history.
