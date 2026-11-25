#!/usr/bin/env bash
# CI only: bootstrap the disposable loopback PostgreSQL 18 service.
set -euo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true || "${PGHOST:-}" != 127.0.0.1 ||
      "${PGUSER:-}" != postgres || "${PGDATABASE:-}" != postgres ||
      "${PGPORT:-5432}" != 5432 || "${PGPASSWORD:-}" != isolated-ci-only ]]; then
  echo "Refusing non-isolated PostgreSQL CI bootstrap" >&2
  exit 1
fi

# GitHub ubuntu-24.04 typically includes psql and pg_isready already.
# Fall back to apt if a future runner image removes either binary.
if command -v psql >/dev/null 2>&1 && command -v pg_isready >/dev/null 2>&1; then
  psql --version
  echo "Using preinstalled PostgreSQL client"
else
  sudo apt-get update -qq
  sudo apt-get install -y postgresql-client
fi
psql -X -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE tor_migrator LOGIN PASSWORD 'isolated-migrate';
CREATE ROLE tor_app LOGIN PASSWORD 'isolated-app';
CREATE DATABASE tor_ci OWNER tor_migrator;
SQL
psql -X -v ON_ERROR_STOP=1 -d tor_ci <<'SQL'
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public GRANT USAGE,SELECT ON SEQUENCES TO tor_app;
SQL
