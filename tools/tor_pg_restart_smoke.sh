#!/usr/bin/env bash
# I-03/I-11: exercise the actual TOR binary against disposable CI PostgreSQL.
# No systemd, production endpoint, VPS, remote S3 or live secrets are touched.
set -Eeuo pipefail

: "${TEST_DATABASE_URL:?requires disposable PostgreSQL fixture}"
: "${PGHOST:?requires CI PostgreSQL administrator connection}"
: "${PGUSER:?requires CI PostgreSQL administrator connection}"
: "${PGPASSWORD:?requires CI PostgreSQL administrator connection}"
: "${PGDATABASE:?requires CI PostgreSQL administrator connection}"

# Never run disruption checks against any non-CI database or an external host.
if [[ "${GITHUB_ACTIONS:-}" != true || "$PGHOST" != 127.0.0.1 || "$PGUSER" != postgres || "$PGDATABASE" != postgres || "$TEST_DATABASE_URL" != postgresql://tor_app:*@127.0.0.1:5432/tor_ci ]]; then
  echo "FAIL: PostgreSQL outage injection is restricted to isolated GitHub Actions tor_ci" >&2
  exit 2
fi

binary="${TOR_RESTART_TEST_BINARY:-./target/debug/tor-web}"
listen="127.0.0.1:3097"
base="http://${listen}"
log_dir="${RUNNER_TEMP:-/tmp}/tor-pg-restart-$$"
mkdir -p "$log_dir"
web_pid=""
role_disabled=0

admin() { psql -X -v ON_ERROR_STOP=1 -q "$@"; }

restore_role() {
  if ((role_disabled)); then
    if ! admin -d postgres -c 'ALTER ROLE tor_app LOGIN' >/dev/null; then
      echo 'FAIL: could not restore disposable CI database login' >&2
      return 1
    fi
    role_disabled=0
  fi
}

stop_web() {
  if [[ -n "$web_pid" ]]; then
    if kill -0 "$web_pid" 2>/dev/null; then
      kill "$web_pid" 2>/dev/null || true
    fi
    wait "$web_pid" 2>/dev/null || true
    web_pid=""
  fi
}

cleanup() {
  local result=$?
  trap - EXIT
  restore_role || result=1
  stop_web
  if ((result != 0)); then
    echo 'TOR PostgreSQL lifecycle test failed; recent application logs:' >&2
    for log in "$log_dir"/*.log; do
      [[ -f "$log" ]] && tail -n 45 "$log" >&2
    done
  fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

require_ready() {
  local n
  for n in $(seq 1 45); do
    if curl -fsS --max-time 2 "$base/readyz" 2>/dev/null | grep -qx ok; then
      return 0
    fi
    if [[ -z "$web_pid" ]] || ! kill -0 "$web_pid" 2>/dev/null; then
      echo 'FAIL: TOR exited before readiness' >&2
      return 1
    fi
    sleep 1
  done
  echo 'FAIL: PostgreSQL readiness did not recover' >&2
  return 1
}

start_web() {
  local label="$1"
  TOR_DEMO_MODE=0 \
    TOR_AUTH_ENABLED=1 \
    TOR_PUBLIC_ORIGIN="$base" \
    TOR_LISTEN_ADDR="$listen" \
    TOR_COVER_WORKER_ENABLED=0 \
    DATABASE_URL="$TEST_DATABASE_URL" \
    "$binary" >"$log_dir/$label.log" 2>&1 &
  web_pid=$!
  require_ready
}

assert_visible_data() {
  local output
  output="$(curl -fsS --max-time 5 "$base/api/v1/books")"
  [[ "$output" == *'TOR Restart Persistence Sentinel'* ]] || {
    echo 'FAIL: persisted sentinel missing from public catalogue' >&2
    return 1
  }
  [[ "$output" != *'TOR CI hidden'* && "$output" != *'TOR CI held'* ]] || {
    echo 'FAIL: private or held book appeared in public catalogue' >&2
    return 1
  }
  [[ "$(curl -sS --max-time 5 -o /dev/null -w '%{http_code}' "$base/books/2")" == 404 ]] || {
    echo 'FAIL: private book became readable' >&2
    return 1
  }
}

[[ -x "$binary" ]] || { echo 'FAIL: build the TOR binary before this test' >&2; exit 1; }
if curl -fsS --max-time 1 "$base/healthz" >/dev/null 2>&1; then
  echo 'FAIL: test loopback address is already in use' >&2
  exit 1
fi

# Explicit background work must not accidentally start without private S3.
if timeout --signal=TERM --kill-after=2s 10s env -u TOR_COVER_S3_BUCKET \
  TOR_DEMO_MODE=0 TOR_COVER_WORKER_ENABLED=1 DATABASE_URL="$TEST_DATABASE_URL" \
  TOR_LISTEN_ADDR="$listen" "$binary" >"$log_dir/invalid-worker.log" 2>&1; then
  echo 'FAIL: S3-less cover worker accepted an invalid startup configuration' >&2
  exit 1
else
  result=$?
  if [[ "$result" == 124 || "$result" == 137 ]]; then
    echo 'FAIL: invalid startup configuration hung instead of failing closed' >&2
    exit 1
  fi
fi
if curl -fsS --max-time 1 "$base/healthz" >/dev/null 2>&1; then
  echo 'FAIL: TOR listened after rejected startup settings' >&2
  exit 1
fi

# Cold boot: check PostgreSQL-backed routes, then create durable user-visible data.
start_web first-start
psql "$TEST_DATABASE_URL" -X -v ON_ERROR_STOP=1 -q <<'SQL'
INSERT INTO books (source_identity, target_language, title, visibility)
SELECT 'ci:startup-restart', 'ru', 'TOR Restart Persistence Sentinel', 'public'
WHERE NOT EXISTS (
  SELECT 1 FROM books
  WHERE source_identity = 'ci:startup-restart' AND target_language = 'ru'
);
SQL
assert_visible_data

echo 'CI_POSTGRES_READY_BEFORE_OUTAGE'
# Block new tor_app logins and terminate existing pooled connections. This changes
# only the ephemeral postgres:18 service database and is ALWAYS reversed by trap.
admin -d postgres -c 'ALTER ROLE tor_app NOLOGIN' >/dev/null
role_disabled=1
admin -d postgres -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename='tor_app' AND datname='tor_ci' AND pid <> pg_backend_pid()" >/dev/null

unready=0
for n in $(seq 1 25); do
  code="$(curl -sS --max-time 3 -o "$log_dir/outage-ready.txt" -w '%{http_code}' "$base/readyz" 2>/dev/null || true)"
  if [[ "$code" == 503 ]] && grep -qx unavailable "$log_dir/outage-ready.txt"; then
    unready=1
    break
  fi
  if ! kill -0 "$web_pid" 2>/dev/null; then
    echo 'FAIL: web process died during transient PostgreSQL outage' >&2
    exit 1
  fi
  sleep 1
done
[[ "$unready" == 1 ]] || { echo 'FAIL: /readyz did not report 503 on lost DB' >&2; exit 1; }
[[ "$(curl -fsS --max-time 3 "$base/healthz")" == ok ]] || {
  echo 'FAIL: /healthz stopped serving during database outage' >&2
  exit 1
}
echo 'CI_POSTGRES_UNREADY_DURING_OUTAGE'

restore_role
require_ready
assert_visible_data
echo 'CI_POSTGRES_READY_AFTER_OUTAGE'

# A fresh process must read the same committed database, not an in-memory demo.
stop_web
start_web after-restart
assert_visible_data
echo 'CI_POSTGRES_RESTART_PERSISTENCE_OK'
