#!/usr/bin/env bash
# Run immediately after fresh disposable migrations, before other fixture suites.
# The browser asserts empty historical windows instead of erasing shared facts.
set -euo pipefail
: "${TEST_DATABASE_URL:?explicit disposable PostgreSQL URL is required}"
export APP_URL="${APP_URL:-$TEST_DATABASE_URL}"
export TOR_AUDIENCE_BASE="${TOR_AUDIENCE_BASE:-http://127.0.0.1:3115}"
export TOR_AUDIENCE_SHOTS="${TOR_AUDIENCE_SHOTS:-/tmp/flow-html-health-shots}"
html_health_log="${TOR_HTML_HEALTH_LOG:-/tmp/flow-html-health-server.log}"
html_health_port="$(node <<'JS'
const assert = require('node:assert/strict');
const db = new URL(process.env.APP_URL);
assert.equal(db.protocol,'postgresql:');
assert.equal(db.hostname,'127.0.0.1');
assert.equal(db.port,'5432');
assert.equal(db.pathname,'/tor_ci');
assert.equal(db.username,'tor_app');
assert.match(process.env.TOR_AUDIENCE_BASE,/^http:\/\/127\.0\.0\.1:[1-9][0-9]*$/);
const base = new URL(process.env.TOR_AUDIENCE_BASE);
assert.ok(Number(base.port)>0 && Number(base.port)<=65535);
process.stdout.write(base.port);
JS
)"
mkdir -p "$TOR_AUDIENCE_SHOTS"
TOR_AUTH_ENABLED=1 TOR_PUBLIC_ORIGIN="$TOR_AUDIENCE_BASE" \
  TOR_LISTEN_ADDR="127.0.0.1:$html_health_port" DATABASE_URL="$APP_URL" \
  "${CARGO_TARGET_DIR:-target}/debug/tor-web" >"$html_health_log" 2>&1 &
html_health_pid=$!
trap 'kill "$html_health_pid" 2>/dev/null || true; wait "$html_health_pid" 2>/dev/null || true' EXIT
for html_health_attempt in $(seq 1 100); do
  if curl --fail --silent "$TOR_AUDIENCE_BASE/readyz" >/dev/null; then break; fi
  kill -0 "$html_health_pid"
  sleep 0.2
done
curl --fail --silent "$TOR_AUDIENCE_BASE/readyz"
node tools/audience_html_health_pg_browser.cjs
