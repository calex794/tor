#!/usr/bin/env bash
# Actual server and commands; only synthetic F history has a private test schema.
set -euo pipefail
: "${TEST_DATABASE_URL:?explicit disposable PostgreSQL URL is required}"
: "${MIGRATOR_URL:?explicit disposable migration owner URL is required}"
export APP_URL="${APP_URL:-$TEST_DATABASE_URL}"
export TOR_CATALOG_INTEREST_BASE="${TOR_CATALOG_INTEREST_BASE:-http://127.0.0.1:3118}"
export TOR_CATALOG_INTEREST_SHOTS="${TOR_CATALOG_INTEREST_SHOTS:-/tmp/catalog-interest-shots}"
export TOR_CATALOG_INTEREST_SCHEMA="flow_catalog_browser_$(date +%s)_$$"
catalog_interest_log="${TOR_CATALOG_INTEREST_LOG:-/tmp/catalog-interest-server.log}"
catalog_interest_pid=''
catalog_interest_schema_created=0
catalog_interest_cleanup() {
  catalog_interest_result=$?
  if [[ -n "$catalog_interest_pid" ]]; then
    kill "$catalog_interest_pid" 2>/dev/null || true
    wait "$catalog_interest_pid" 2>/dev/null || true
  fi
  if [[ "$catalog_interest_schema_created" == 1 ]]; then
    node tools/catalog_interest_pg_fixture.cjs cleanup || catalog_interest_result=1
  fi
  exit "$catalog_interest_result"
}
trap catalog_interest_cleanup EXIT
# This helper validates both loopback URLs, database, ports and expected roles
# before its first connection. tor_app receives SELECT only on all F fixture data.
node tools/catalog_interest_pg_fixture.cjs prepare
catalog_interest_schema_created=1
catalog_interest_url="$(node tools/catalog_interest_pg_fixture.cjs url)"
catalog_interest_port="$(node -e 'process.stdout.write(new URL(process.env.TOR_CATALOG_INTEREST_BASE).port)')"
mkdir -p "$TOR_CATALOG_INTEREST_SHOTS"
TOR_AUTH_ENABLED=1 TOR_PUBLIC_ORIGIN="$TOR_CATALOG_INTEREST_BASE" \
  TOR_FLOW_OBSERVATIONS_ENABLED=0 TOR_LISTEN_ADDR="127.0.0.1:$catalog_interest_port" \
  DATABASE_URL="$catalog_interest_url" "${CARGO_TARGET_DIR:-target}/debug/tor-web" >"$catalog_interest_log" 2>&1 &
catalog_interest_pid=$!
for catalog_interest_attempt in $(seq 1 100); do
  if curl --fail --silent "$TOR_CATALOG_INTEREST_BASE/readyz" >/dev/null; then break; fi
  kill -0 "$catalog_interest_pid"
  sleep 0.2
done
curl --fail --silent "$TOR_CATALOG_INTEREST_BASE/readyz"
node tools/catalog_interest_pg_browser.cjs
