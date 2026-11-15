#!/usr/bin/env bash
# Real public C writer plus an isolated SELECT-only C report and deterministic test clock.
set -Eeuo pipefail
: "${TEST_DATABASE_URL:?disposable PostgreSQL required}"
: "${MIGRATOR_URL:?disposable migration owner required}"
export APP_URL="${APP_URL:-$TEST_DATABASE_URL}"
export TOR_READING_TIME_BASE="${TOR_READING_TIME_BASE:-http://127.0.0.1:3120}"
export TOR_READING_TIME_WRITER_BASE="${TOR_READING_TIME_WRITER_BASE:-http://127.0.0.1:3119}"
export TOR_READING_TIME_SHOTS="${TOR_READING_TIME_SHOTS:-/tmp/reading-time-shots}"
export TOR_READING_TIME_SCHEMA="flow_reading_time_browser_$(date +%s)_$$"
reading_time_log="${TOR_READING_TIME_LOG_PREFIX:-/tmp/server-reading-time}"
reading_time_writer_pid=''
reading_time_report_pid=''
reading_time_schema_created=0
reading_time_cleanup() {
  reading_time_result=$?
  for reading_time_pid in "$reading_time_report_pid" "$reading_time_writer_pid"; do
    if [[ -n "$reading_time_pid" ]]; then
      kill "$reading_time_pid" 2>/dev/null || true
      wait "$reading_time_pid" 2>/dev/null || true
    fi
  done
  if [[ "$reading_time_schema_created" == 1 ]]; then
    node tools/reading_time_pg_fixture.cjs cleanup || reading_time_result=1
  fi
  if (( reading_time_result != 0 )); then
    for reading_time_tail in "$reading_time_log-writer.log" "$reading_time_log-report.log"; do
      if [[ -f "$reading_time_tail" ]]; then tail -n 60 "$reading_time_tail" >&2; fi
    done
  fi
  exit "$reading_time_result"
}
trap reading_time_cleanup EXIT
node tools/reading_time_pg_fixture.cjs prepare
reading_time_schema_created=1
reading_time_report_url="$(node tools/reading_time_pg_fixture.cjs url)"
reading_time_writer_port="$(node -e 'process.stdout.write(new URL(process.env.TOR_READING_TIME_WRITER_BASE).port)')"
reading_time_report_port="$(node -e 'process.stdout.write(new URL(process.env.TOR_READING_TIME_BASE).port)')"
mkdir -p "$TOR_READING_TIME_SHOTS" "$(dirname "$reading_time_log")"
TOR_AUTH_ENABLED=1 TOR_PUBLIC_ORIGIN="$TOR_READING_TIME_WRITER_BASE" \
  TOR_FLOW_OBSERVATIONS_ENABLED=0 TOR_FLOW_SEO_IMPORT=0 \
  TOR_LISTEN_ADDR="127.0.0.1:$reading_time_writer_port" DATABASE_URL="$APP_URL" \
  "${CARGO_TARGET_DIR:-target}/debug/tor-web" >"$reading_time_log-writer.log" 2>&1 &
reading_time_writer_pid=$!
TOR_AUTH_ENABLED=1 TOR_PUBLIC_ORIGIN="$TOR_READING_TIME_BASE" \
  TOR_FLOW_OBSERVATIONS_ENABLED=0 TOR_FLOW_SEO_IMPORT=0 \
  TOR_LISTEN_ADDR="127.0.0.1:$reading_time_report_port" DATABASE_URL="$reading_time_report_url" \
  "${CARGO_TARGET_DIR:-target}/debug/tor-web" >"$reading_time_log-report.log" 2>&1 &
reading_time_report_pid=$!
for reading_time_attempt in $(seq 1 100); do
  if curl --fail --silent "$TOR_READING_TIME_WRITER_BASE/readyz" >/dev/null &&
     curl --fail --silent "$TOR_READING_TIME_BASE/readyz" >/dev/null; then break; fi
  kill -0 "$reading_time_writer_pid"
  kill -0 "$reading_time_report_pid"
  sleep 0.2
done
curl --fail --silent "$TOR_READING_TIME_WRITER_BASE/readyz"
curl --fail --silent "$TOR_READING_TIME_BASE/readyz"
node tools/reading_time_pg_browser.cjs
