#!/usr/bin/env bash
# I-11/S061: verify a PostgreSQL 18 backup with a real isolated restore and
# a real TOR HTTP process. Run ONLY in GitHub Actions' disposable tor_ci.
set -Eeuo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true ||
      "${PGHOST:-}" != 127.0.0.1 ||
      "${PGUSER:-}" != postgres ||
      "${PGDATABASE:-}" != postgres ||
      "${TEST_DATABASE_URL:-}" != 'postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci' ||
      "${MIGRATOR_URL:-}" != 'postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci' ]]; then
  echo 'REFUSE: backup drill is restricted to disposable tor_ci in GitHub Actions' >&2
  exit 2
fi
: "${PGPASSWORD:?missing test-only superuser password}"
: "${RUNNER_TEMP:?GitHub Actions temporary directory required}"

restore_db=tor_restore_ci
app_url='postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_restore_ci'
web_binary="${TOR_RESTORE_TEST_BINARY:-./target/debug/tor-web}"
base='http://127.0.0.1:3096'
work_dir="$(mktemp -d "$RUNNER_TEMP/tor-pg-restore.XXXXXXXX")"
chmod 700 "$work_dir"
web_pid=""
created=0

admin() { psql -X -v ON_ERROR_STOP=1 -q -d postgres "$@"; }

cleanup() {
  local result=$?
  trap - EXIT
  if [[ -n "$web_pid" ]]; then
    kill "$web_pid" 2>/dev/null || true
    wait "$web_pid" 2>/dev/null || true
  fi
  if ((created)); then
    admin -c "DROP DATABASE $restore_db WITH (FORCE)" >/dev/null || result=1
  fi
  # The temporary dump must not be uploaded as an artifact or retained.
  rm -f -- "$work_dir/tor-ci.dump" "$work_dir/backup-contents.txt" "$work_dir/tor-web.log"
  rmdir -- "$work_dir" || result=1
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[[ -x "$web_binary" ]] || { echo 'FAIL: TOR binary not built' >&2; exit 1; }
if curl -fsS --max-time 1 "$base/healthz" >/dev/null 2>&1; then
  echo 'FAIL: isolated HTTP port already in use' >&2
  exit 1
fi
if admin -At -c "SELECT 1 FROM pg_database WHERE datname='$restore_db'" | grep -qx 1; then
  echo 'FAIL: restore target already exists, never overwrite it' >&2
  exit 1
fi

# Seed distinct public and private rows so this drill tests actual data.
psql "$TEST_DATABASE_URL" -X -v ON_ERROR_STOP=1 -q <<'SQL'
INSERT INTO books (source_identity, target_language, title, visibility)
VALUES ('ci:i11:backup-public', 'ru', 'TOR Backup Restore Canary', 'public')
ON CONFLICT (source_identity, target_language) DO NOTHING;
INSERT INTO books (source_identity, target_language, title, visibility)
VALUES ('ci:i11:backup-private', 'ru', 'TOR Backup Private Canary', 'draft')
ON CONFLICT (source_identity, target_language) DO NOTHING;
SQL

source_history="$(psql "$MIGRATOR_URL" -X -At -v ON_ERROR_STOP=1 \
  -c "SELECT count(*) || ':' || max(version) FROM _sqlx_migrations WHERE success=true")"
[[ "$source_history" =~ ^[1-9][0-9]*:[0-9]+$ ]] || {
  echo 'FAIL: source database lacks successful SQLx migration history' >&2
  exit 1
}

# Host psql can be older than PostgreSQL 18. Use the server's major version
# of pg_dump/pg_restore from the existing CI postgres:18 image.
docker run --rm --network host \
  --user "$(id -u):$(id -g)" \
  -e PGPASSWORD=tor-ci-migrate \
  -v "$work_dir:/backup:rw" \
  postgres:18 pg_dump \
    --host=127.0.0.1 --port=5432 --username=tor_migrator \
    --dbname=tor_ci --format=custom --no-owner --file=/backup/tor-ci.dump

[[ -s "$work_dir/tor-ci.dump" ]] || {
  echo 'FAIL: pg_dump returned an empty archive' >&2
  exit 1
}
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$work_dir:/backup:ro" postgres:18 \
  pg_restore --list /backup/tor-ci.dump > "$work_dir/backup-contents.txt"
grep -q '_sqlx_migrations' "$work_dir/backup-contents.txt" || {
  echo 'FAIL: archived SQLx schema history was missing' >&2
  exit 1
}
echo 'I11_PG18_BACKUP_ARCHIVE_OK'

# Restore into a NEW database; never switch or overwrite the original tor_ci.
admin -c "CREATE DATABASE $restore_db OWNER tor_migrator" >/dev/null
created=1
admin -c "GRANT CONNECT ON DATABASE $restore_db TO tor_app" >/dev/null

docker run --rm --network host \
  --user "$(id -u):$(id -g)" \
  -e PGPASSWORD=tor-ci-migrate \
  -v "$work_dir:/backup:ro" \
  postgres:18 pg_restore \
    --host=127.0.0.1 --port=5432 --username=tor_migrator \
    --dbname="$restore_db" --no-owner --exit-on-error \
    /backup/tor-ci.dump

# Do not weaken runtime privileges to pass the restore test.
restored_history="$(psql "$app_url" -X -At -v ON_ERROR_STOP=1 \
  -c "SELECT count(*) || ':' || max(version) FROM _sqlx_migrations WHERE success=true")"
[[ "$source_history" == "$restored_history" ]] || {
  echo 'FAIL: restored SQLx migration history differs from the original' >&2
  exit 1
}
if psql "$app_url" -X -v ON_ERROR_STOP=1 -q \
  -c 'CREATE TABLE public.i11_forbidden_ddl (id integer)' >/dev/null 2>&1; then
  echo 'FAIL: restored app role acquired unintended CREATE privilege' >&2
  exit 1
fi

public_id="$(psql "$app_url" -X -At -v ON_ERROR_STOP=1 \
  -c "SELECT id FROM books WHERE source_identity='ci:i11:backup-public' AND visibility='public'")"
private_id="$(psql "$app_url" -X -At -v ON_ERROR_STOP=1 \
  -c "SELECT id FROM books WHERE source_identity='ci:i11:backup-private' AND visibility='draft'")"
[[ "$public_id" =~ ^[0-9]+$ && "$private_id" =~ ^[0-9]+$ ]] || {
  echo 'FAIL: restored public or private record differs from source' >&2
  exit 1
}

TOR_DEMO_MODE=0 TOR_AUTH_ENABLED=1 \
  TOR_PUBLIC_ORIGIN="$base" TOR_LISTEN_ADDR=127.0.0.1:3096 \
  TOR_COVER_WORKER_ENABLED=0 DATABASE_URL="$app_url" \
  "$web_binary" > "$work_dir/tor-web.log" 2>&1 &
web_pid=$!

ready=0
for attempt in $(seq 1 45); do
  if curl -fsS --max-time 2 "$base/readyz" 2>/dev/null | grep -qx ok; then
    ready=1
    break
  fi
  if ! kill -0 "$web_pid" 2>/dev/null; then
    break
  fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then
  echo 'FAIL: restored database did not pass actual TOR /readyz' >&2
  tail -n 35 "$work_dir/tor-web.log" >&2
  exit 1
fi
public_page="$(curl -fsS --max-time 5 "$base/books/$public_id")"
[[ "$public_page" == *'TOR Backup Restore Canary'* ]] || {
  echo 'FAIL: restored public book not delivered via HTTP' >&2
  exit 1
}
private_code="$(curl -sS --max-time 5 -o /dev/null -w '%{http_code}' "$base/books/$private_id")"
[[ "$private_code" == 404 ]] || {
  echo 'FAIL: draft became readable after restore' >&2
  exit 1
}
echo 'I11_PG18_RESTORE_HTTP_ACL_OK'

# Also check that the source remains intact after the independent restore.
source_id="$(psql "$TEST_DATABASE_URL" -X -At -v ON_ERROR_STOP=1 \
  -c "SELECT id FROM books WHERE source_identity='ci:i11:backup-public'")"
[[ "$source_id" == "$public_id" ]] || {
  echo 'FAIL: source database changed or identifiers diverged' >&2
  exit 1
}
echo 'I11_PG18_RESTORE_SOURCE_UNCHANGED_OK'
