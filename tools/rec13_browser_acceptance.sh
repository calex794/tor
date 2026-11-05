#!/usr/bin/env bash
# Real Chromium + loopback TOR + disposable PostgreSQL. Never touch VPS.
set -Eeuo pipefail
: "${TEST_DATABASE_URL:?disposable PostgreSQL required}"
if [[ "${GITHUB_ACTIONS:-}" != true || "$TEST_DATABASE_URL" != postgresql://tor_app:*@127.0.0.1:5432/tor_ci ]]; then
  echo 'REC13 browser acceptance is restricted to isolated GitHub Actions tor_ci' >&2
  exit 2
fi
root="${RUNNER_TEMP:-/tmp}/rec13-browser"
mkdir -p "$root/shots" "$root/node"
exec > >(tee "$root/browser.log") 2>&1
cargo build -p tor-web
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
TOR_AUTH_ENABLED=1 TOR_RECOMMENDATIONS_ENABLED=1 TOR_REC_ATTRIBUTION_ENABLED=1 TOR_ORGANIC_ATTRIBUTION_ENABLED=1 \
  TOR_PUBLIC_ORIGIN=http://127.0.0.1:3108 TOR_LISTEN_ADDR=127.0.0.1:3108 \
  DATABASE_URL="$TEST_DATABASE_URL" ./target/debug/tor-web >"$root/server.log" 2>&1 &
pid=$!
cleanup() {
  result=$?
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  if (( result != 0 )); then tail -n 60 "$root/server.log" >&2; fi
  exit "$result"
}
trap cleanup EXIT
for attempt in $(seq 1 45); do
  if curl --fail --silent http://127.0.0.1:3108/readyz >/dev/null; then break; fi
  kill -0 "$pid"
  sleep 1
 done
curl --fail --silent http://127.0.0.1:3108/readyz
NODE_PATH="$node_path" APP_URL="$TEST_DATABASE_URL" \
  TOR_REC_BASE=http://127.0.0.1:3108 TOR_REC_SHOTS="$root/shots" node tools/rec13_browser.cjs
