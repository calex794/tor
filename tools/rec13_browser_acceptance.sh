#!/usr/bin/env bash
# Real Chromium + loopback TOR + disposable PostgreSQL. Never touch VPS.
set -Eeuo pipefail
: "${TEST_DATABASE_URL:?disposable PostgreSQL required}"
if [[ "${GITHUB_ACTIONS:-}" != true || "$TEST_DATABASE_URL" != postgresql://tor_app:*@127.0.0.1:5432/tor_ci ]]; then
  echo 'REC13 browser acceptance is restricted to isolated GitHub Actions tor_ci' >&2
  exit 2
fi
# Bounded screenshots from the actual CI checkout supplement the full PNG artifacts.
export TOR_BROWSER_PROOF_IMAGES=1
root="${RUNNER_TEMP:-/tmp}/rec13-browser"
mkdir -p "$root/shots" "$root/node"
exec > >(tee "$root/browser.log") 2>&1
# TIMING: measure required steps in both standalone and reused-browser modes.
ci_rec_started=$SECONDS
ci_rec_step_done() {
  local stage="$1"
  echo "CI_REC13_STEP_SECONDS stage=$stage seconds=$((SECONDS-ci_rec_started))"
  ci_rec_started=$SECONDS
}
cargo build -p tor-web
ci_rec_step_done "cargo_build"
# The mandatory PostgreSQL job has already installed Playwright and the
# headless Chromium shell. Reuse them only when explicitly requested; keep
# this acceptance script self-contained for its independent workflow.
if [[ "${TOR_REC_REUSE_PLAYWRIGHT:-0}" == 1 ]]; then
  # Playwright's executablePath() points to *headed* Chromium, which is
  # intentionally absent in --only-shell mode. Launch the real headless shell
  # instead, so missing browser dependencies fail before starting the test.
  node -e "const p=require('playwright'); if (require('playwright/package.json').version !== '1.56.1') process.exit(1); p.chromium.launch({headless:true}).then(b=>b.close()).catch(e=>{console.error(e);process.exit(1)})"
  node_path="$PWD/node_modules"
else
  npm install --prefix "$root/node" --no-save playwright@1.56.1
  # Standalone REC acceptance uses the same real headless Chromium as the
  # mandatory PG job. Avoid optional Ubuntu fonts downloaded via apt mirrors.
  "$root/node/node_modules/.bin/playwright" install --only-shell chromium
  node_path="$root/node/node_modules"
fi
ci_rec_step_done "playwright_setup"
: "${LEGACY_ANALYTICS_DATABASE_URL:?explicit legacy 192 database required}"
: "${COLLECTION_RETIREMENT_APP_DATABASE_URL:?installed 190 to current upgrade database required}"
pid=''
stop_server() {
  if [[ -n "$pid" ]]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    pid=''
  fi
}
cleanup() {
  result=$?
  stop_server
  if (( result != 0 )); then
    for server_log in "$root"/server-*.log; do
      if [[ -f "$server_log" ]]; then tail -n 60 "$server_log" >&2; fi
    done
  fi
  exit "$result"
}
trap cleanup EXIT
start_server() {
  local fixture_url="$1"
  local fixture_name="$2"
  local surprises_optin="${3:-0}"
  local surprise_test_key=''
  if [[ "$surprises_optin" == 1 ]]; then
    surprise_test_key="$(node -e "process.stdout.write(require('node:crypto').randomBytes(32).toString('base64url'))")"
  fi
  TOR_SURPRISE_HTTP_ENABLED="$surprises_optin" TOR_SURPRISE_HMAC_KEY="$surprise_test_key" \
    TOR_AUTH_ENABLED=1 TOR_FLOW_VISITS_ENABLED=1 TOR_READING_RETURNS_ENABLED=1 TOR_BOOK_READING_RETURNS_ENABLED=1 TOR_RECOMMENDATIONS_ENABLED=1 TOR_REC_ATTRIBUTION_ENABLED=1 TOR_ORGANIC_ATTRIBUTION_ENABLED=1 \
    TOR_PUBLIC_ORIGIN=http://127.0.0.1:3108 TOR_LISTEN_ADDR=127.0.0.1:3108 \
    DATABASE_URL="$fixture_url" ./target/debug/tor-web >"$root/server-$fixture_name.log" 2>&1 &
  pid=$!
  for attempt in $(seq 1 45); do
    if curl --fail --silent http://127.0.0.1:3108/readyz >/dev/null; then break; fi
    kill -0 "$pid"
    sleep 1
  done
  curl --fail --silent http://127.0.0.1:3108/readyz
  if [[ "$fixture_name" == current ]]; then
    ci_rec_step_done "http_ready"
  else
    ci_rec_step_done "http_ready_$fixture_name"
  fi
}
# Current business behavior is always exercised behind the installed SQL barrier.
start_server "$TEST_DATABASE_URL" current
NODE_PATH="$node_path" APP_URL="$TEST_DATABASE_URL" \
  TOR_REC_BASE=http://127.0.0.1:3108 TOR_REC_SHOTS="$root/shots" node tools/rec13_browser.cjs
ci_rec_step_done "browser_acceptance"
NODE_PATH="$node_path" APP_URL="$TEST_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/audience_pg_browser.cjs
ci_rec_step_done "audience_browser_acceptance"
NODE_PATH="$node_path" APP_URL="$TEST_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/audience_hours_pg_browser.cjs
ci_rec_step_done "audience_hours_browser_acceptance"
NODE_PATH="$node_path" APP_URL="$TEST_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/flow_visits_pg_browser.cjs
ci_rec_step_done "flow_visits_browser_acceptance"
NODE_PATH="$node_path" APP_URL="$TEST_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/flow_funnel_pg_browser.cjs
ci_rec_step_done "flow_funnel_browser_acceptance"
NODE_PATH="$node_path" APP_URL="$TEST_DATABASE_URL" \
  TOR_REC_BASE=http://127.0.0.1:3108 TOR_REC_SHOTS="$root/shots" \
  TOR_PREFERENCES_PROOF_IMAGES=1 node tools/recommendation_preferences_pg_browser.cjs
ci_rec_step_done "recommendation_preferences_browser_acceptance"
stop_server
# The full optional gift UI runs only against disposable tor_ci. Never enable
# the feature on the regular CI server, a legacy fixture or any live installation.
start_server "$TEST_DATABASE_URL" surprises 1
NODE_PATH="$node_path" TEST_DATABASE_URL="$TEST_DATABASE_URL" \
  TOR_SURPRISE_BASE=http://127.0.0.1:3108 TOR_SURPRISE_SHOTS="$root/shots" \
  node tools/surprise_pg_browser.cjs
ci_rec_step_done "surprise_chromium_acceptance"
stop_server
# Historical collector/report arithmetic remains executable in its own 192 database.
# No current-schema trigger is disabled and no application collection bypass is added.
start_server "$LEGACY_ANALYTICS_DATABASE_URL" legacy192
NODE_PATH="$node_path" APP_URL="$LEGACY_ANALYTICS_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/audience_sources_pg_browser.cjs
ci_rec_step_done "legacy192_audience_sources_browser_acceptance"
NODE_PATH="$node_path" APP_URL="$LEGACY_ANALYTICS_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/audience_books_pg_browser.cjs
ci_rec_step_done "legacy192_audience_books_browser_acceptance"
NODE_PATH="$node_path" APP_URL="$LEGACY_ANALYTICS_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/audience_groups_pg_browser.cjs
ci_rec_step_done "legacy192_audience_groups_browser_acceptance"
NODE_PATH="$node_path" APP_URL="$LEGACY_ANALYTICS_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/audience_returns_pg_browser.cjs
ci_rec_step_done "legacy192_audience_returns_browser_acceptance"
NODE_PATH="$node_path" APP_URL="$LEGACY_ANALYTICS_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/audience_book_returns_pg_browser.cjs
ci_rec_step_done "legacy192_audience_book_returns_browser_acceptance"
NODE_PATH="$node_path" APP_URL="$LEGACY_ANALYTICS_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/audience_group_returns_pg_browser.cjs
ci_rec_step_done "legacy192_audience_group_returns_browser_acceptance"
stop_server
# This database was genuinely upgraded from accepted 190 and drained by the bounded port.
start_server "$COLLECTION_RETIREMENT_APP_DATABASE_URL" installed-upgrade
NODE_PATH="$node_path" APP_URL="$COLLECTION_RETIREMENT_APP_DATABASE_URL" \
  TOR_AUDIENCE_BASE=http://127.0.0.1:3108 TOR_AUDIENCE_SHOTS="$root/shots" node tools/analytics_retirement_pg_browser.cjs
ci_rec_step_done "installed_retirement_browser_acceptance"
stop_server
