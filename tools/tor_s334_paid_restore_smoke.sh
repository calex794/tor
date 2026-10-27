#!/usr/bin/env bash
# I-11/S3-34: paid immutable V1 and PostgreSQL + private S3 + independent KEK.
# Only disposable GitHub Actions tor_ci, MinIO loopback and restored CI database.
set -Eeuo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true ||
      "${PGHOST:-}" != 127.0.0.1 ||
      "${PGUSER:-}" != postgres ||
      "${PGDATABASE:-}" != postgres ||
      "${TEST_DATABASE_URL:-}" != 'postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci' ||
      "${MIGRATOR_URL:-}" != 'postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci' ||
      "${S334_RESTORED_DATABASE_URL:-}" != 'postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_s334_restore_ci' ||
      "${TOR_TEST_S3_ENDPOINT:-}" != http://127.0.0.1:9000 ||
      "${TOR_TEST_S3_ACCESS_KEY:-}" != tor-chapter-ci-user ||
      "${TOR_TEST_S3_SECRET_KEY:-}" != tor-chapter-ci-secret-only ]]; then
  echo 'REFUSE: S3-34 is for disposable PG18 + loopback MinIO only' >&2
  exit 2
fi
: "${PGPASSWORD:?missing disposable admin connection}"
: "${RUNNER_TEMP:?missing GitHub Actions temporary directory}"
: "${GITHUB_SHA:?missing exact GitHub commit}"

bucket=tor-private-chapter-ci
backup=tor-s334-backup-ci
id=f0c0e034-2026-4034-8034-000000000134
key="private/chapter-versions/$id.enc"
backup_key="s334/$key"
object="s334/$GITHUB_SHA/pg18.dump.aesgcm"
endpoint="$TOR_TEST_S3_ENDPOINT"
work="$(mktemp -d "$RUNNER_TEMP/tor-s334.XXXXXXXX")"
chmod 700 "$work"
restore_db=tor_s334_restore_ci
created=0

admin() { psql -X -v ON_ERROR_STOP=1 -q -d postgres "$@"; }
s3() {
  curl --fail --silent --show-error --max-time 30 \
    --aws-sigv4 'aws:amz:us-east-1:s3' \
    --user "$TOR_TEST_S3_ACCESS_KEY:$TOR_TEST_S3_SECRET_KEY" "$@"
}
cleanup() {
  local code=$?
  trap - EXIT
  if ((created)); then
    admin -c "DROP DATABASE $restore_db WITH (FORCE)" >/dev/null || code=1
  fi
  rm -f -- "$work"/*
  rmdir -- "$work" || code=1
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[[ -s "$RUNNER_TEMP/s334-state.json" && -s "$RUNNER_TEMP/s334-content-kek.bin" ]] || {
  echo 'FAIL: paid purchase fixture or independent KEK absent' >&2
  exit 1
}
if admin -At -c "SELECT 1 FROM pg_database WHERE datname='$restore_db'" | grep -qx 1; then
  echo 'REFUSE: existing database must never be overwritten' >&2
  exit 1
fi
source_history="$(psql "$MIGRATOR_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c 'SELECT concat(count(*),chr(58),max(version)) FROM _sqlx_migrations WHERE success')"
[[ "$source_history" =~ ^[1-9][0-9]*:[0-9]+$ ]] || exit 1
sha="$(psql "$TEST_DATABASE_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c "SELECT encode(sha256_ciphertext,'hex') FROM storage_objects WHERE id='$id'::uuid AND state='attached' AND kek_id='content-kek-v1'")"
[[ "$sha" =~ ^[0-9a-f]{64}$ ]] || {
  echo 'FAIL: pinned encrypted V1 not attached in source PostgreSQL' >&2
  exit 1
}

# Back up both parts: SQLx/Purchases/PG storage registry + immutable ciphertext.
# KEK remains a third independent local recovery input and is NEVER uploaded.
docker run --rm --network host \
  --user "$(id -u):$(id -g)" -e PGPASSWORD=tor-ci-migrate \
  -v "$work:/work:rw" postgres:18 \
  pg_dump --host=127.0.0.1 --port=5432 --username=tor_migrator \
  --dbname=tor_ci --format=custom --no-owner --file=/work/source.dump
[[ -s "$work/source.dump" ]] || exit 1
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$work:/work:ro" postgres:18 \
  pg_restore --list /work/source.dump > "$work/catalog.txt"
grep -q '_sqlx_migrations' "$work/catalog.txt"
grep -q 'storage_objects' "$work/catalog.txt"

s3 --request PUT --output /dev/null "$endpoint/$backup"
s3 --output "$work/source-v1.enc" "$endpoint/$bucket/$key"
[[ "$(sha256sum "$work/source-v1.enc" | cut -d ' ' -f 1)" == "$sha" ]] || {
  echo 'FAIL: source object bytes do not match PostgreSQL registry' >&2
  exit 1
}
s3 --upload-file "$work/source-v1.enc" \
  --header 'If-None-Match: *' \
  --header 'Content-Type: application/octet-stream' \
  --output /dev/null "$endpoint/$backup/$backup_key"

node tools/tor_dr_archive_crypto.cjs new-key "$work/backup.key"
node tools/tor_dr_archive_crypto.cjs seal "$work/backup.key" \
  "$work/source.dump" "$work/db.enc"
pg_digest="$(sha256sum "$work/db.enc" | cut -d ' ' -f 1)"
s3 --upload-file "$work/db.enc" \
  --header 'If-None-Match: *' \
  --header 'Content-Type: application/octet-stream' \
  --output /dev/null "$endpoint/$backup/$object"
for k in "$backup_key" "$object"; do
  code="$(curl --silent --max-time 3 --output /dev/null \
    --write-out '%{http_code}' "$endpoint/$backup/$k")"
  [[ "$code" == 403 ]] || { echo 'FAIL: S3 backup is anonymously accessible' >&2; exit 1; }
done
echo 'S334_PG_S3_CIPHERTEXT_BACKUP_READY'

# Simulate real loss of both local archive and the original private S3 object.
# The backup bucket remains; source tor_ci is never dropped or overwritten.
s3 --request DELETE --output /dev/null "$endpoint/$bucket/$key"
missing="$(curl --silent --max-time 5 --output /dev/null -w '%{http_code}' \
  --aws-sigv4 'aws:amz:us-east-1:s3' \
  --user "$TOR_TEST_S3_ACCESS_KEY:$TOR_TEST_S3_SECRET_KEY" \
  "$endpoint/$bucket/$key")"
[[ "$missing" == 404 ]] || { echo 'FAIL: original private V1 did not disappear'; exit 1; }
rm -f -- "$work/source.dump" "$work/source-v1.enc" "$work/db.enc"
s3 --output "$work/retrieved-db.enc" "$endpoint/$backup/$object"
[[ "$(sha256sum "$work/retrieved-db.enc" | cut -d ' ' -f 1)" == "$pg_digest" ]] || {
  echo 'FAIL: independent SQL backup corrupted' >&2
  exit 1
}
node tools/tor_dr_archive_crypto.cjs open "$work/backup.key" \
  "$work/retrieved-db.enc" "$work/restored.dump"
echo 'S334_PRIVATE_RETRIEVED_PG18_ARCHIVE_OK'

admin -c "CREATE DATABASE $restore_db OWNER tor_migrator" >/dev/null
created=1
admin -c "GRANT CONNECT ON DATABASE $restore_db TO tor_app" >/dev/null
docker run --rm --network host \
  --user "$(id -u):$(id -g)" -e PGPASSWORD=tor-ci-migrate \
  -v "$work:/work:ro" postgres:18 \
  pg_restore --host=127.0.0.1 --port=5432 --username=tor_migrator \
  --dbname="$restore_db" --no-owner --exit-on-error /work/restored.dump

history="$(psql "$S334_RESTORED_DATABASE_URL" -X -A -t -v ON_ERROR_STOP=1 \
  -c 'SELECT concat(count(*),chr(58),max(version)) FROM _sqlx_migrations WHERE success')"
[[ "$history" == "$source_history" ]] || {
  echo 'FAIL: restored database migration history differs' >&2
  exit 1
}
if psql "$S334_RESTORED_DATABASE_URL" -X -v ON_ERROR_STOP=1 -q \
  -c 'CREATE TABLE public.s334_unexpected_ddl (n integer)' >/dev/null 2>&1; then
  echo 'FAIL: restored application role gained DDL rights' >&2
  exit 1
fi

# Rust must first reject missing S3, then recover the exact V1 ciphertext from
# the backup bucket, reject missing/wrong KEK, verify D/B/G rights, issue V1.
cargo test -p tor-web --features s3-paid \
  --test s334_paid_restore_pg_s3 s3_34_verify_restored_paid_v1_and_rejected_keys \
  -- --ignored --exact --test-threads=1

# The browser-compatible decryption must work with the *new* scoped grant
# issued after recovery, never with a cached pre-disaster bearer URL.
TOR_PAID_GRANT_FIXTURE_PATH="$RUNNER_TEMP/s334-webcrypto.json" \
  node tools/paid_grant_webcrypto_e2e.mjs
echo 'S334_PURCHASED_V1_AFTER_V2_PG_S3_KEK_RESTORE_PASS'
