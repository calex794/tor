#!/usr/bin/env bash
# PostgreSQL 18 integration smoke, used only with disposable CI test DBs.
set -Eeuo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

# Match SQLx's exact leading marker. Ordinary contiguous files retain their
# existing shared transaction; each marked file gets its own psql invocation.
# Explicit failure propagation also applies when this function is tested in if.
apply() {
  local db=$1; shift
  local migration first_line
  local args=()
  for migration in "$@"; do
    first_line=$(head -n 1 -- "$migration") || return
    if [[ $first_line == '-- no-transaction'* ]]; then
      if (( ${#args[@]} )); then
        psql -X -v ON_ERROR_STOP=1 -1 -d "$db" "${args[@]}" || return
        args=()
      fi
      psql -X -v ON_ERROR_STOP=1 -d "$db" -f "$migration" || return
    else
      args+=(-f "$migration")
    fi
  done
  if (( ${#args[@]} )); then
    psql -X -v ON_ERROR_STOP=1 -1 -d "$db" "${args[@]}" || return
  fi
}

main() {
  python3 tools/check_migration_sources.py
  : "${PGHOST:?}"
  : "${PGUSER:?}"
  : "${PGPASSWORD:?}"

  psql -X -v ON_ERROR_STOP=1 -d postgres -c 'CREATE DATABASE tor_migrations_fresh'
  psql -X -v ON_ERROR_STOP=1 -d postgres -c 'CREATE DATABASE tor_migrations_upgrade'

  local migrations
  mapfile -t migrations < <(find migrations -maxdepth 1 -name '*.sql' -printf '%p\n' | LC_ALL=C sort)
  if (( ${#migrations[@]} < 10 )); then
    echo 'Expected nine pinned SQL migrations and additive TOR migrations' >&2
    exit 1
  fi

  # Fresh installation.
  apply tor_migrations_fresh "${migrations[@]}"
  psql -X -v ON_ERROR_STOP=1 -d tor_migrations_fresh -f tests/postgres/migrations-smoke.sql

  # Data installed before the remaining additive migrations must survive.
  apply tor_migrations_upgrade "${migrations[@]:0:6}"
  psql -X -v ON_ERROR_STOP=1 -d tor_migrations_upgrade <<'SQL'
INSERT INTO books(source_identity,target_language,title,visibility)
  VALUES ('ci-preexisting', 'ru', 'Existing book', 'public');
SQL
  apply tor_migrations_upgrade "${migrations[@]:6}"
  psql -X -v ON_ERROR_STOP=1 -d tor_migrations_upgrade -Atqc \
    "SELECT count(*) FROM books WHERE source_identity='ci-preexisting'" | grep -qx '1'
  psql -X -v ON_ERROR_STOP=1 -d tor_migrations_upgrade -f tests/postgres/migrations-smoke.sql

  bash tests/postgres/verify-migration-modes.sh
  echo 'PG18: fresh SQL migrations PASS, upgrade with existing data PASS, privacy/constraints PASS'
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  main "$@"
fi
