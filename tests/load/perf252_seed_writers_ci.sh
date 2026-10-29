#!/usr/bin/env bash
# PERF-252: disposable local-only sessions; never point at a real database.
set -euo pipefail
if [[ "${GITHUB_EVENT_NAME:-}" != pull_request ||
      "${PGHOST:-}" != 127.0.0.1 ||
      "${TEST_DATABASE_URL:-}" != */tor_ci ]]; then
  echo "PERF-252 refused: requires a local PR tor_ci database" >&2
  exit 1
fi
identity="$(psql -X -A -t -q "$TEST_DATABASE_URL" -c 'SELECT current_database(),current_user')"
if [[ "$identity" != 'tor_ci|tor_app' ]]; then
  echo "PERF-252 refused: incorrect PostgreSQL identity" >&2
  exit 1
fi
psql -X -v ON_ERROR_STOP=1 "$TEST_DATABASE_URL" <<'SQL'
BEGIN;
-- Accounts and sessions are created through real HTTP /register calls.
-- A longer free chapter permits monotonically advancing progress per reader.
INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
VALUES(401,1,2,'PERF-252 progress chapter','public','free',0);
INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
VALUES(501,401,1,repeat('P',4000));
INSERT INTO chapter_publications(chapter_id,version_id) VALUES(401,501);
COMMIT;
SQL
psql -X -A -t -q "$TEST_DATABASE_URL" -c "SELECT count(*) FROM chapter_publications WHERE chapter_id=401 AND version_id=501" | grep -qx 1
