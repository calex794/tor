#!/usr/bin/env bash
# I-11/S061: independently stored ENCRYPTED PG18 backup -> clean restore.
# CI-only. The S3-compatible vault is a separate MinIO process on loopback,
# not a production/off-site durability guarantee.
set -Eeuo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true ||
      "${PGHOST:-}" != 127.0.0.1 ||
      "${PGUSER:-}" != postgres ||
      "${PGDATABASE:-}" != postgres ||
      "${TEST_DATABASE_URL:-}" != 'postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci' ||
      "${MIGRATOR_URL:-}" != 'postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci' ||
      "${TOR_DR_S3_ENDPOINT:-}" != http://127.0.0.1:9010 ||
      "${TOR_DR_S3_ACCESS_KEY:-}" != tor-dr-ci-user ||
      "${TOR_DR_S3_SECRET_KEY:-}" != tor-dr-ci-secret-only ]]; then
  echo 'REFUSE: I-11 S3 restore drill requires isolated CI PostgreSQL and MinIO' >&2
  exit 2
fi
: "${PGPASSWORD:?missing disposable CI PG admin password}"
: "${RUNNER_TEMP:?missing disposable CI work directory}"
: "${GITHUB_SHA:?missing exact CI source SHA}"

binary="${TOR_DR_WEB_BINARY:-./target/debug/tor-web}"
bucket=tor-private-backup-ci
object="backup/${GITHUB_SHA:0:12}/pg18.dump.aesgcm"
endpoint="$TOR_DR_S3_ENDPOINT"
work="$(mktemp -d "$RUNNER_TEMP/tor-dr-vault.XXXXXXXX")"
chmod 700 "$work"
restore_db=tor_offsite_restore_ci
restored_url='postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_offsite_restore_ci'
base=http://127.0.0.1:3095
web_pid=""
created=0

admin() { psql -X -v ON_ERROR_STOP=1 -q -d postgres "$@"; }
s3() {
  curl --silent --show-error --fail --max-time 30 \
    --aws-sigv4 'aws:amz:us-east-1:s3' \
    --user "$TOR_DR_S3_ACCESS_KEY:$TOR_DR_S3_SECRET_KEY" "$@"
}

cleanup() {
  local status=$?
  trap - EXIT
  if [[ -n "$web_pid" ]]; then
    kill "$web_pid" 2>/dev/null || true
    wait "$web_pid" 2>/dev/null || true
  fi
  if ((created)); then
    admin -c "DROP DATABASE $restore_db WITH (FORCE)" >/dev/null || status=1
  fi
  # No artifact uploads: all plaintext and ciphertext are CI fixture only.
  rm -f -- "$work"/*
  rmdir -- "$work" || status=1
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[[ -x "$binary" ]] || { echo 'FAIL: TOR binary not built' >&2; exit 1; }
curl --help all | grep -q -- '--aws-sigv4' || {
  echo 'FAIL: curl with AWS Signature V4 support required' >&2
  exit 1
}
if admin -At -c "SELECT 1 FROM pg_database WHERE datname='$restore_db'" | grep -qx 1; then
  echo 'REFUSE: separate restore database already exists' >&2
  exit 1
fi
if curl -fsS --max-time 2 "$base/healthz" >/dev/null 2>&1; then
  echo 'REFUSE: restore web port already in use' >&2
  exit 1
fi

# A committed public and a private row must BOTH survive the backup.
psql "$TEST_DATABASE_URL" -X -v ON_ERROR_STOP=1 -q <<'SQL'
INSERT INTO books(source_identity,target_language,title,visibility)
VALUES('ci:dr:public','ru','TOR Independent Backup Test','public')
ON CONFLICT(source_identity,target_language) DO NOTHING;
INSERT INTO books(source_identity,target_language,title,visibility)
VALUES('ci:dr:hidden','ru','TOR Private Backup Test','draft')
ON CONFLICT(source_identity,target_language) DO NOTHING;
SQL
history="$(psql "$MIGRATOR_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c 'SELECT count(*)::text || ':' || max(version)::text FROM _sqlx_migrations WHERE success')"
[[ "$history" =~ ^[1-9][0-9]*:[0-9]+$ ]] || {
  echo 'FAIL: SQLx migration history unavailable' >&2
  exit 1
}

docker run --rm --network host \
  --user "$(id -u):$(id -g)" \
  -e PGPASSWORD=tor-ci-migrate \
  -v "$work:/work:rw" postgres:18 \
  pg_dump --host=127.0.0.1 --port=5432 --username=tor_migrator \
  --dbname=tor_ci --format=custom --no-owner --file=/work/source.dump
[[ -s "$work/source.dump" ]] || {
  echo 'FAIL: no PostgreSQL 18 archive' >&2
  exit 1
}
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$work:/work:ro" postgres:18 \
  pg_restore --list /work/source.dump >"$work/catalog.txt"
grep -q '_sqlx_migrations' "$work/catalog.txt"

node tools/tor_dr_archive_crypto.cjs new-key "$work/offline.key"
node tools/tor_dr_archive_crypto.cjs seal "$work/offline.key" \
  "$work/source.dump" "$work/archive.enc"
digest="$(sha256sum "$work/archive.enc" | cut -d ' ' -f 1)"
[[ "$digest" =~ ^[a-f0-9]{64}$ ]]
s3 --request PUT --output /dev/null "$endpoint/$bucket"
s3 --upload-file "$work/archive.enc" \
  --header 'If-None-Match: *' \
  --header 'Content-Type: application/octet-stream' \
  --output /dev/null "$endpoint/$bucket/$object"

anon_code="$(curl --silent --max-time 5 --output /dev/null \
  --write-out '%{http_code}' "$endpoint/$bucket/$object")"
[[ "$anon_code" == 403 ]] || {
  echo 'FAIL: anonymous S3 backup archive is not private' >&2
  exit 1
}
missing_code="$(curl --silent --max-time 5 --output /dev/null \
  --write-out '%{http_code}' --aws-sigv4 'aws:amz:us-east-1:s3' \
  --user "$TOR_DR_S3_ACCESS_KEY:$TOR_DR_S3_SECRET_KEY" \
  "$endpoint/$bucket/backup/absent.dump.aesgcm")"
[[ "$missing_code" == 404 ]] || {
  echo 'FAIL: missing S3 archive must not masquerade as successful recovery' >&2
  exit 1
}

# Remove BOTH local copies. The next stage can only recover bytes from S3;
# the independent recovery key is intentionally NOT uploaded to S3/PG.
rm -f -- "$work/source.dump" "$work/archive.enc"
s3 --output "$work/downloaded.enc" "$endpoint/$bucket/$object"
[[ "$(sha256sum "$work/downloaded.enc" | cut -d ' ' -f 1)" == "$digest" ]] || {
  echo 'FAIL: recovered S3 object digest changed' >&2
  exit 1
}

node tools/tor_dr_archive_crypto.cjs new-key "$work/wrong.key"
if node tools/tor_dr_archive_crypto.cjs open "$work/wrong.key" \
    "$work/downloaded.enc" "$work/wrong.dump"; then
  echo 'FAIL: wrong recovery key unexpectedly decrypted PG dump' >&2
  exit 1
fi
[[ ! -e "$work/wrong.dump" ]]
node tools/tor_dr_archive_crypto.cjs damage \
  "$work/downloaded.enc" "$work/corrupt.enc"
if node tools/tor_dr_archive_crypto.cjs open "$work/offline.key" \
    "$work/corrupt.enc" "$work/corrupt.dump"; then
  echo 'FAIL: tampered S3 backup unexpectedly decrypted' >&2
  exit 1
fi
[[ ! -e "$work/corrupt.dump" ]]
node tools/tor_dr_archive_crypto.cjs open "$work/offline.key" \
  "$work/downloaded.enc" "$work/restored.dump"
echo 'I11_PRIVATE_S3_ENCRYPTED_BACKUP_VERIFIED'

admin -c "CREATE DATABASE $restore_db OWNER tor_migrator" >/dev/null
created=1
admin -c "GRANT CONNECT ON DATABASE $restore_db TO tor_app" >/dev/null
docker run --rm --network host \
  --user "$(id -u):$(id -g)" -e PGPASSWORD=tor-ci-migrate \
  -v "$work:/work:ro" postgres:18 \
  pg_restore --host=127.0.0.1 --port=5432 --username=tor_migrator \
  --dbname="$restore_db" --no-owner --exit-on-error /work/restored.dump

restored_history="$(psql "$restored_url" -X -A -t -v ON_ERROR_STOP=1 \
  -c 'SELECT count(*)::text || ':' || max(version)::text FROM _sqlx_migrations WHERE success')"
[[ "$history" == "$restored_history" ]] || {
  echo 'FAIL: restored schema history differs' >&2
  exit 1
}
if psql "$restored_url" -X -v ON_ERROR_STOP=1 \
  -c 'CREATE TABLE public.forbidden_dr_ddl (x integer)' >/dev/null 2>&1; then
  echo 'FAIL: restored tor_app is permitted to write schema' >&2
  exit 1
fi
pub_id="$(psql "$restored_url" -X -A -t -v ON_ERROR_STOP=1 \
  -c "SELECT id FROM books WHERE source_identity='ci:dr:public' AND visibility='public'")"
hidden_id="$(psql "$restored_url" -X -A -t -v ON_ERROR_STOP=1 \
  -c "SELECT id FROM books WHERE source_identity='ci:dr:hidden' AND visibility='draft'")"
[[ "$pub_id" =~ ^[0-9]+$ && "$hidden_id" =~ ^[0-9]+$ ]]

TOR_DEMO_MODE=0 TOR_AUTH_ENABLED=1 \
  TOR_PUBLIC_ORIGIN="$base" TOR_LISTEN_ADDR=127.0.0.1:3095 \
  TOR_COVER_WORKER_ENABLED=0 DATABASE_URL="$restored_url" \
  "$binary" >"$work/tor-web.log" 2>&1 &
web_pid=$!
healthy=0
for attempt in $(seq 1 50); do
  if curl -fsS --max-time 2 "$base/readyz" 2>/dev/null | grep -qx ok; then
    healthy=1
    break
  fi
  if ! kill -0 "$web_pid" 2>/dev/null; then break; fi
  sleep 1
done
if [[ "$healthy" != 1 ]]; then
  echo 'FAIL: TOR could not start on remotely restored PostgreSQL' >&2
  tail -n 25 "$work/tor-web.log" >&2
  exit 1
fi
page="$(curl --fail --silent --show-error --max-time 5 "$base/books/$pub_id")"
[[ "$page" == *'TOR Independent Backup Test'* ]] || {
  echo 'FAIL: restored book missing from true HTTP response' >&2
  exit 1
}
private_code="$(curl --silent --max-time 5 --output /dev/null \
  --write-out '%{http_code}' "$base/books/$hidden_id")"
[[ "$private_code" == 404 ]] || {
  echo 'FAIL: restored private book became public' >&2
  exit 1
}
echo 'I11_S3_PG18_RESTORED_AXUM_ACL_OK'
