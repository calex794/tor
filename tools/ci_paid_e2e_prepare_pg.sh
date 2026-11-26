#!/usr/bin/env bash
# CI-only disposable PostgreSQL and private loopback S3 fixtures.
set -euo pipefail
if [[ "${GITHUB_ACTIONS:-}" != true || "${PGHOST:-}" != 127.0.0.1 ||
      "${PGDATABASE:-}" != postgres ||
      "${TEST_DATABASE_URL:-}" != postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci ||
      "${MIGRATOR_URL:-}" != postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci ]]; then
  echo 'Refusing non-isolated paid E2E fixture' >&2
  exit 2
fi
set -euo pipefail
psql -X -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE tor_migrator LOGIN PASSWORD 'tor-ci-migrate';
CREATE ROLE tor_app LOGIN PASSWORD 'tor-ci-app';
CREATE DATABASE tor_ci OWNER tor_migrator;
SQL
psql -X -v ON_ERROR_STOP=1 -d tor_ci <<'SQL'
REVOKE ALL ON DATABASE tor_ci FROM PUBLIC;
GRANT CONNECT ON DATABASE tor_ci TO tor_app;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
  GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
  GRANT USAGE,SELECT ON SEQUENCES TO tor_app;
SQL
DATABASE_URL="$MIGRATOR_URL" cargo run -p tor-migrations --bin tor-migrate
