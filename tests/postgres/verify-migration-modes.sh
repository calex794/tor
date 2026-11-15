#!/usr/bin/env bash
# Real PostgreSQL transaction/marker regression, disposable CI database only.
set -Eeuo pipefail
cd "$(dirname "$0")/../.."
source tests/postgres/verify-migrations.sh
: "${PGHOST:?}"
: "${PGUSER:?}"
: "${PGPASSWORD:?}"
[[ $PGHOST == 127.0.0.1 && ${PGPORT:-5432} == 5432 ]] || exit 2
fixture_dir=$(mktemp -d)
trap 'rm -rf "$fixture_dir"' EXIT
fixture_db=tor_migrations_modes
psql -X -v ON_ERROR_STOP=1 -d postgres -c "CREATE DATABASE $fixture_db"

cat >"$fixture_dir/ordinary-a.sql" <<'SQL'
CREATE TABLE must_rollback(id integer);
INSERT INTO must_rollback VALUES(1);
SQL
cat >"$fixture_dir/ordinary-b.sql" <<'SQL'
INSERT INTO must_rollback VALUES(2);
SELECT 1 / 0;
SQL
if apply "$fixture_db" "$fixture_dir/ordinary-a.sql" "$fixture_dir/ordinary-b.sql"; then
  echo 'Ordinary transaction unexpectedly accepted a failing file' >&2
  exit 1
fi
[[ $(psql -X -v ON_ERROR_STOP=1 -d "$fixture_db" -Atqc \
  "SELECT to_regclass('must_rollback') IS NULL") == t ]]

cat >"$fixture_dir/before.sql" <<'SQL'
CREATE TABLE kept(id integer);
INSERT INTO kept VALUES(1);
SQL
cat >"$fixture_dir/concurrent.sql" <<'SQL'
-- no-transaction
CREATE INDEX CONCURRENTLY kept_id ON kept(id);
SQL
cat >"$fixture_dir/after-a.sql" <<'SQL'
INSERT INTO kept VALUES(2);
SQL
cat >"$fixture_dir/after-b.sql" <<'SQL'
INSERT INTO kept VALUES(3);
SELECT 1 / 0;
SQL
if apply "$fixture_db" "$fixture_dir/before.sql" "$fixture_dir/concurrent.sql" \
    "$fixture_dir/after-a.sql" "$fixture_dir/after-b.sql"; then
  echo 'Trailing ordinary batch unexpectedly accepted a failing file' >&2
  exit 1
fi
[[ $(psql -X -v ON_ERROR_STOP=1 -d "$fixture_db" -Atqc \
  'SELECT count(*)=1 AND min(id)=1 FROM kept') == t ]]
[[ $(psql -X -v ON_ERROR_STOP=1 -d "$fixture_db" -Atqc \
  "SELECT indisvalid AND indisready FROM pg_index WHERE indexrelid='kept_id'::regclass") == t ]]

cat >"$fixture_dir/marked-failure.sql" <<'SQL'
-- no-transaction
CREATE INDEX CONCURRENTLY forbidden_index ON absent_relation(id);
SQL
cat >"$fixture_dir/never.sql" <<'SQL'
CREATE TABLE must_not_run(id integer);
SQL
if apply "$fixture_db" "$fixture_dir/marked-failure.sql" "$fixture_dir/never.sql"; then
  echo 'Marked failure was ignored' >&2
  exit 1
fi
[[ $(psql -X -v ON_ERROR_STOP=1 -d "$fixture_db" -Atqc \
  "SELECT to_regclass('must_not_run') IS NULL") == t ]]
echo 'FLOW_MIGRATION_MODES_PASS: ordinary rollback, marked commit, ordered failure propagation'
