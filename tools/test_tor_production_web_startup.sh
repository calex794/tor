#!/usr/bin/env bash
# S061: actual release executable, isolated PostgreSQL and local MinIO only.
# Never inherit operator configuration or access a live provider.
set -euo pipefail
expected_db='postgresql://tor_app:tor-obs09-app@127.0.0.1:5432/tor_ci'
[[ "${GITHUB_ACTIONS:-}" == true && "${TEST_DATABASE_URL:-}" == "$expected_db" &&
   -n "${RUNNER_TEMP:-}" && -d "$RUNNER_TEMP" ]] || {
  echo 'TOR_PRODUCTION_SMOKE_REFUSED: disposable CI only' >&2
  exit 2
}
log=$(mktemp "$RUNNER_TEMP/tor-production-startup.XXXXXXXX")
port=3098
pid=""
cleanup() {
  if [[ -n "$pid" ]]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT
# Actual release binary must start with the cover, free chapter and import
# buckets present. Paid/worker flags remain off (no cloud writes or cutover).
fixture=(
  PATH="$PATH" HOME="$RUNNER_TEMP"
  DATABASE_URL="$expected_db" TOR_DEMO_MODE=0 TOR_AUTH_ENABLED=1
  TOR_LISTEN_ADDR="127.0.0.1:$port" TOR_PUBLIC_ORIGIN=https://example.invalid
  AWS_EC2_METADATA_DISABLED=true AWS_REGION=ru-central1
  AWS_ENDPOINT_URL_S3=http://127.0.0.1:9000
  AWS_ACCESS_KEY_ID=tor-cover-ci-user AWS_SECRET_ACCESS_KEY=tor-cover-ci-secret-only
  TOR_COVER_S3_BUCKET=tor-private-obs09-ci
  TOR_CHAPTER_IMPORT_S3_BUCKET=tor-private-cover-ci
  TOR_FREE_S3_BUCKET=tor-private-obs09-ci TOR_PAID_S3_BUCKET=tor-private-cover-ci
  TOR_COVER_WORKER_ENABLED=0 TOR_PAID_GRANTS_ENABLED=0
  TOR_PAID_AUTHOR_PUBLISH_ENABLED=0 TOR_PAID_AUTHOR_S3_ONLY_ENABLED=0
)
# Reproduce the first failed production-profile CI run. Require a named,
# secret-free error for a configured FREE bucket without the reader origin.
status=0
timeout 15s env -i "${fixture[@]}" target/release/tor-web > "$log" 2>&1 || status=$?
if [[ "$status" != 1 ]] || ! grep -Fq 'Missing or invalid TOR_FREE_S3_PUBLIC_ORIGIN' "$log"; then
  tail -n 60 "$log" >&2
  echo 'TOR_PRODUCTION_SMOKE_REFUSED: missing origin was not diagnosed' >&2
  exit 1
fi
echo 'TOR_PRODUCTION_MISSING_FREE_ORIGIN_REFUSED'
# CI uses only the known loopback MinIO origin. Never guess the production URL.
env -i "${fixture[@]}" TOR_FREE_S3_PUBLIC_ORIGIN=http://127.0.0.1:9000 \
  target/release/tor-web > "$log" 2>&1 &
pid=$!
ready=0
for attempt in $(seq 1 60); do
  if ! kill -0 "$pid" 2>/dev/null; then break; fi
  if [[ $(curl -fsS --max-time 2 "http://127.0.0.1:$port/readyz" 2>/dev/null || true) == ok ]]; then
    ready=1
    break
  fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then
  # Only synthetic CI secrets and test data in this isolated process.
  tail -n 60 "$log" >&2
  echo 'TOR_PRODUCTION_SMOKE_REFUSED: full-feature release did not start' >&2
  exit 1
fi
curl -fsS --max-time 10 "http://127.0.0.1:$port/catalog" >/dev/null
login_html=$(curl -fsS --max-time 10 "http://127.0.0.1:$port/login")
[[ "$login_html" == *'name="csrf_token"'* ]]
echo 'TOR_PRODUCTION_WEB_STARTUP_OK: PostgreSQL and configured S3 adapters'
