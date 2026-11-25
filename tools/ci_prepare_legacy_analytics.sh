#!/usr/bin/env bash
# CI only: create the historical analytics fixtures on a disposable PostgreSQL service.
set -euo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true || "${PGHOST:-}" != 127.0.0.1 ||
      "${PGUSER:-}" != postgres || "${PGDATABASE:-}" != postgres ||
      "${PGPORT:-5432}" != 5432 ||
      ( "${PGPASSWORD:-}" != isolated-ci-only && "${PGPASSWORD:-}" != tor-ci-postgres ) ]]; then
  echo "Refusing non-isolated legacy analytics CI bootstrap" >&2
  exit 1
fi

psql -X -v ON_ERROR_STOP=1 -d postgres <<'SQL'
CREATE DATABASE tor_ci_legacy_analytics OWNER tor_migrator;
CREATE DATABASE tor_ci_collection_retirement_upgrade OWNER tor_migrator;
CREATE DATABASE tor_ci_collection_retirement_startup OWNER tor_migrator;
SQL
for fixture_database in tor_ci_legacy_analytics tor_ci_collection_retirement_upgrade tor_ci_collection_retirement_startup; do
  psql -X -v ON_ERROR_STOP=1 -v fixture_database="$fixture_database" -d "$fixture_database" <<'SQL'
REVOKE ALL ON DATABASE :"fixture_database" FROM PUBLIC;
GRANT CONNECT ON DATABASE :"fixture_database" TO tor_app;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
  GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
  GRANT USAGE,SELECT ON SEQUENCES TO tor_app;
SQL
done
