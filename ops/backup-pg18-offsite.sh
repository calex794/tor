#!/usr/bin/env bash
# TOR I-11 / DR-01: operator-installed, encrypted off-site PostgreSQL 18 backup.
# This script NEVER configures credentials, schedules itself or touches live data in CI.
set -Eeuo pipefail
umask 077

fail() { printf 'TOR_OFFSITE_BACKUP_REFUSED: %s\n' "$*" >&2; exit 1; }
[[ "$(id -u)" == 0 ]] || fail 'root required'
[[ "$#" == 0 ]] || fail 'no command-line arguments or credentials allowed'

config=/etc/tor/offsite-backup.env
[[ -f "$config" && ! -L "$config" ]] || fail 'root-owned configuration missing'
[[ "$(stat -c '%U:%G:%a' "$config")" == root:root:600 ]] ||
  fail 'configuration must be root:root mode 600'
# shellcheck disable=SC1091
source "$config"

[[ "${RESTIC_REPOSITORY:-}" == s3:https://* ]] ||
  fail 'repository must use remote HTTPS S3; no local or HTTP repository'
[[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]] ||
  fail 'dedicated S3 credentials required'
[[ -n "${RESTIC_PASSWORD_FILE:-}" &&
   -f "$RESTIC_PASSWORD_FILE" && ! -L "$RESTIC_PASSWORD_FILE" ]] ||
  fail 'recovery password file missing or a symlink'
[[ "$(stat -c '%U:%G:%a' "$RESTIC_PASSWORD_FILE")" == root:root:600 ]] ||
  fail 'recovery password file must be root:root mode 600'
[[ -s "$RESTIC_PASSWORD_FILE" ]] || fail 'empty recovery password'
export RESTIC_REPOSITORY RESTIC_PASSWORD_FILE AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY

for executable in restic pg_isready runuser flock stat; do
  command -v "$executable" >/dev/null || fail "missing dependency: $executable"
done
pgdump=/usr/lib/postgresql/18/bin/pg_dump
[[ -x "$pgdump" ]] || fail 'PostgreSQL 18 pg_dump required'
[[ -d /var/run/postgresql ]] || fail 'local PostgreSQL socket missing'
pg_isready -q -h /var/run/postgresql -p 5432 -d tor ||
  fail 'local TOR PostgreSQL 18 is not accepting connections'

# Serialize a single backup run even when a timer and operator overlap.
exec 9>/run/lock/tor-offsite-backup.lock
flock -n 9 || fail 'another off-site backup is running'

# Do not silently initialize a new repository after credentials or URL change.
restic snapshots --json --tag tor-pg18 >/dev/null ||
  fail 'existing restic repository cannot be opened'
# --stdin-from-command propagates pg_dump errors to the snapshot operation.
# No plaintext PostgreSQL dump is written to disk. Repository is encrypted.
restic backup --stdin-from-command --stdin-filename tor-pg18.dump \
  --tag tor-pg18 -- \
  runuser -u postgres -- "$pgdump" \
  --host=/var/run/postgresql --port=5432 --username=postgres \
  --dbname=tor --format=custom --no-owner --no-acl
echo TOR_OFFSITE_BACKUP_OK
