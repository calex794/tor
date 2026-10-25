#!/usr/bin/env bash
# PostgreSQL 18 integration smoke, used only with disposable CI test DBs.
set -Eeuo pipefail
cd "$(dirname "$0")/../.."
python3 tools/check_migration_sources.py

: "${PGHOST:?}"
: "${PGUSER:?}"
: "${PGPASSWORD:?}"

psql -X -v ON_ERROR_STOP=1 -d postgres -c 'CREATE DATABASE tor_migrations_fresh'
psql -X -v ON_ERROR_STOP=1 -d postgres -c 'CREATE DATABASE tor_migrations_upgrade'

mapfile -t migrations < <(find migrations -maxdepth 1 -name '*.sql' -printf '%f\n' | LC_ALL=C sort)
if (( ${#migrations[@]} != 9 )); then
  echo 'Expected exactly nine pinned SQL migration files' >&2
  exit 1
fi

apply() {
  local db=$1; shift
  local m
  local args=()
  for m in "$@"; do
    args+=(-f "migrations/$m")
  done
  psql -X -v ON_ERROR_STOP=1 -1 -d "$db" "${args[@]}"
}

# Fresh installation.
apply tor_migrations_fresh "${migrations[@]}"
psql -X -v ON_ERROR_STOP=1 -d tor_migrations_fresh -f tests/postgres/migrations-smoke.sql

# Existing data after migration 006 must survive the remaining three migrations.
apply tor_migrations_upgrade "${migrations[@]:0:6}"
psql -X -v ON_ERROR_STOP=1 -d tor_migrations_upgrade <<'SQL'
INSERT INTO books(source_identity,target_language,title,visibility)
  VALUES ('ci-preexisting', 'ru', 'Existing book', 'public');
SQL
apply tor_migrations_upgrade "${migrations[@]:6:3}"
psql -X -v ON_ERROR_STOP=1 -d tor_migrations_upgrade -Atqc \
  "SELECT count(*) FROM books WHERE source_identity='ci-preexisting'" | grep -qx '1'
psql -X -v ON_ERROR_STOP=1 -d tor_migrations_upgrade -f tests/postgres/migrations-smoke.sql

echo 'PG18: fresh SQL migrations PASS, upgrade with existing data PASS, privacy/constraints PASS'
