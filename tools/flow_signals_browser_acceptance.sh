#!/usr/bin/env bash
# Development/integration browser gate. The caller has migrated disposable PG,
# built this exact checkout and installed Playwright Chromium with its full deps.
set -euo pipefail
: "${TEST_DATABASE_URL:?explicit disposable PostgreSQL URL is required}"
export APP_URL="${APP_URL:-$TEST_DATABASE_URL}"
export TOR_SIGNALS_BASE="${TOR_SIGNALS_BASE:-http://127.0.0.1:3114}"
export TOR_SIGNALS_SHOTS="${TOR_SIGNALS_SHOTS:-/tmp/flow-signals-shots}"
signals_log="${TOR_SIGNALS_LOG:-/tmp/flow-signals-server.log}"
# Validate before starting any application/background worker.
signals_port="$(node <<'JS'
const assert = require('node:assert/strict');
const db = new URL(process.env.APP_URL);
const port = process.env.FLOW_TEST_PG_PORT || '5432';
assert.match(port,/^[1-9][0-9]{0,4}$/);
assert.ok(Number(port)<=65535);
assert.equal(db.protocol,'postgresql:');
assert.equal(db.hostname,'127.0.0.1');
assert.equal(db.port,port);
assert.equal(db.pathname,'/tor_ci');
assert.equal(db.username,'tor_app');
assert.match(process.env.TOR_SIGNALS_BASE,/^http:\/\/127\.0\.0\.1:[1-9][0-9]*$/);
const base = new URL(process.env.TOR_SIGNALS_BASE);
assert.ok(Number(base.port)>0 && Number(base.port)<=65535);
process.stdout.write(base.port);
JS
)"
mkdir -p "$TOR_SIGNALS_SHOTS"
TOR_AUTH_ENABLED=1 TOR_PUBLIC_ORIGIN="$TOR_SIGNALS_BASE" \
  TOR_LISTEN_ADDR="127.0.0.1:$signals_port" DATABASE_URL="$APP_URL" \
  "${CARGO_TARGET_DIR:-target}/debug/tor-web" >"$signals_log" 2>&1 &
signals_server_pid=$!
trap 'kill "$signals_server_pid" 2>/dev/null || true; wait "$signals_server_pid" 2>/dev/null || true' EXIT
for signals_attempt in $(seq 1 100); do
  if curl --fail --silent "$TOR_SIGNALS_BASE/readyz" >/dev/null; then break; fi
  kill -0 "$signals_server_pid"
  sleep 0.2
done
curl --fail --silent "$TOR_SIGNALS_BASE/readyz"
node tools/flow_signals_pg_browser.cjs
# Only these two freshly generated fixture screenshots are emitted. No storage
# state, headers, cookies or authentication tokens are printed.
node <<'JS'
const fs = require('node:fs');
const path = require('node:path');
for (const width of [390,1440]) {
  const name = 'signals-review-' + width + '.png';
  const data = fs.readFileSync(path.join(process.env.TOR_SIGNALS_SHOTS,name));
  console.log('FLOW_VISUAL_BEGIN ' + JSON.stringify({path:name,mime:'image/png',bytes:data.length,width,height:900}));
  const encoded = data.toString('base64');
  for(let offset=0;offset<encoded.length;offset+=3000) console.log('FLOW_VISUAL_DATA ' + encoded.slice(offset,offset+3000));
  console.log('FLOW_VISUAL_END');
}
JS
