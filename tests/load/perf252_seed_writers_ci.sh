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
-- No password login is performed; these are disposable test-only sessions.
INSERT INTO users(email_normalized,display_name,password_hash)
SELECT 'ciw'||lpad(n::text,4,'0')||'@example.invalid',
       'PERF-252 reader '||n,'$argon2id$fixture'
FROM generate_series(1,128) AS n;
-- sha256(bytea) is built into PostgreSQL 18. Only the digest is persisted.
INSERT INTO sessions(token_hash,user_id,expires_at)
SELECT sha256(convert_to(repeat('S',35) ||
       lpad(substring(email_normalized from 4 for 4),8,'0'),'UTF8')),
       id,now()+interval '1 hour'
FROM users WHERE email_normalized LIKE 'ciw____@example.invalid';
-- A longer free chapter permits monotonically advancing progress per reader.
INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
VALUES(401,1,2,'PERF-252 progress chapter','public','free',0);
INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
VALUES(501,401,1,repeat('P',4000));
INSERT INTO chapter_publications(chapter_id,version_id) VALUES(401,501);
COMMIT;
SQL
psql -X -A -t -q "$TEST_DATABASE_URL" -c "
  SELECT count(*) FROM sessions s JOIN users u ON u.id=s.user_id
  WHERE u.email_normalized LIKE 'ciw____@example.invalid'
" | grep -qx 128
