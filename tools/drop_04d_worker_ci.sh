#!/usr/bin/env bash
# DROP-04d: a fresh queue for the real worker; do not consume older fixtures.
set -Eeuo pipefail
if [[ ${GITHUB_ACTIONS:-} != true || ${PGHOST:-} != 127.0.0.1 ||
      ${PGUSER:-} != postgres || ${PGDATABASE:-} != postgres ||
      ${TEST_DATABASE_URL:-} != postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci ||
      ${MIGRATOR_URL:-} != postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci ||
      ${TOR_TEST_S3_ENDPOINT:-} != http://127.0.0.1:9000 ]]; then
  echo 'REFUSE: DROP worker requires disposable CI PostgreSQL and MinIO' >&2
  exit 2
fi
psql -X -v ON_ERROR_STOP=1 -c 'CREATE DATABASE tor_drop_worker_ci OWNER tor_migrator'
psql -X -v ON_ERROR_STOP=1 -d tor_drop_worker_ci <<'SQL'
REVOKE ALL ON DATABASE tor_drop_worker_ci FROM PUBLIC;
GRANT CONNECT ON DATABASE tor_drop_worker_ci TO tor_app;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
  GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
  GRANT USAGE,SELECT ON SEQUENCES TO tor_app;
SQL
DATABASE_URL="${MIGRATOR_URL%/tor_ci}/tor_drop_worker_ci" cargo run -p tor-migrations --bin tor-migrate
export TEST_DATABASE_URL="${TEST_DATABASE_URL%/tor_ci}/tor_drop_worker_ci"
cargo test -p tor-web --features s3-free,s3-chapter-import --test drop_04d_worker_pg_minio -- --ignored --test-threads=1 --show-output --skip drop_04d_100k_bounded_worker_import
TOR_DROP_LOAD_CHAPTERS=100000 cargo test -p tor-web --features s3-free,s3-chapter-import --test drop_04d_worker_pg_minio drop_04d_100k_bounded_worker_import -- --ignored --test-threads=1 --show-output
