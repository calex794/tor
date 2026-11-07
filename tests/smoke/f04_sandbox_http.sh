#!/usr/bin/env bash
# F-04 end-to-end HTML smoke against a disposable loopback-only PostgreSQL site.
# Never touches VPS, external hosts, real users, or production storage.
set -euo pipefail

[[ -n "$DATABASE_URL" && -n "$RUNNER_TEMP" ]] || exit 2
[[ "$DATABASE_URL" == postgresql://tor_app:*@127.0.0.1:5432/tor_ci ]] || {
  echo "F04_SANDBOX_REFUSED: expected disposable tor_app@127.0.0.1/tor_ci" >&2
  exit 2
}
[[ "$PGHOST" == 127.0.0.1 && "$PGDATABASE" == tor_ci && "$PGUSER" == tor_app ]] || {
  echo "F04_SANDBOX_REFUSED: PostgreSQL must be local tor_ci with tor_app" >&2
  exit 2
}

base=http://127.0.0.1:3089
output="$RUNNER_TEMP/f04-sandbox"
mkdir -p "$output"
server_pid=""
stop_server() {
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
    server_pid=""
  fi
}
trap stop_server EXIT

start_server() {
  local enabled="$1" ready=0
  TOR_RISING_SHELF_ENABLED="$enabled" \
  TOR_LISTEN_ADDR=127.0.0.1:3089 \
  TOR_AUTH_ENABLED=0 \
  TOR_DEMO_MODE=0 \
  TOR_PAID_CHECKOUT_ENABLED=0 \
  ./target/release/tor-web > "$output/server-$enabled.log" 2>&1 &
  server_pid=$!
  for _ in $(seq 1 60); do
    if curl --silent --fail --max-time 2 "$base/readyz" | grep -qx ok; then
      ready=1
      break
    fi
    if ! kill -0 "$server_pid" 2>/dev/null; then
      break
    fi
    sleep 1
  done
  if [[ "$ready" != 1 ]]; then
    echo "F04_SANDBOX_FAIL: server did not become ready" >&2
    tail -n 50 "$output/server-$enabled.log" >&2
    exit 1
  fi
}

get_home() {
  local file="$1"
  curl --silent --show-error --fail --max-time 10 "$base/" -o "$file"
}
assert_absent() {
  local file="$1"
  if grep -Fq 'id="home-rising"' "$file"; then
    echo "F04_SANDBOX_FAIL: rising shelf unexpectedly visible: $file" >&2
    exit 1
  fi
}
assert_present() {
  local file="$1"
  grep -Fq 'id="home-rising"' "$file" || {
    echo "F04_SANDBOX_FAIL: growing shelf missing: $file" >&2
    exit 1
  }
  grep -Fq 'Набирают читателей' "$file" || exit 1
  grep -Fq 'href="/books/987610"' "$file" || {
    echo "F04_SANDBOX_FAIL: no canonical card link" >&2
    exit 1
  }
  grep -Fq 'F04 Sandbox Rising Book' "$file" || exit 1
}

# Flag OFF: the book may appear in "recently added", but not the rising shelf.
start_server 0
get_home "$output/disabled.html"
assert_absent "$output/disabled.html"
stop_server
echo F04_SANDBOX_FLAG_OFF_PASS

# Flag ON: verify real HTTP home response and public book route.
start_server 1
get_home "$output/enabled.html"
assert_present "$output/enabled.html"
curl --silent --show-error --fail --max-time 10 "$base/books/987610" \
  -o "$output/book.html"
grep -Fq 'F04 Sandbox Rising Book' "$output/book.html" || exit 1
echo F04_SANDBOX_FLAG_ON_PASS

# Already-running process must re-check current access instead of stale cache.
psql -X -v ON_ERROR_STOP=1 -c \
  "UPDATE users SET account_state='suspended' WHERE id=987603" >/dev/null
get_home "$output/suspended.html"
assert_absent "$output/suspended.html"
psql -X -v ON_ERROR_STOP=1 -c \
  "UPDATE users SET account_state='active' WHERE id=987603" >/dev/null
get_home "$output/restored.html"
assert_present "$output/restored.html"
echo F04_SANDBOX_REVOKED_READER_PASS

psql -X -v ON_ERROR_STOP=1 -c \
  "UPDATE books SET admin_hold=TRUE WHERE id=987610" >/dev/null
get_home "$output/held.html"
assert_absent "$output/held.html"
psql -X -v ON_ERROR_STOP=1 -c \
  "UPDATE books SET admin_hold=FALSE WHERE id=987610" >/dev/null
get_home "$output/clear.html"
assert_present "$output/clear.html"
echo F04_SANDBOX_REVOKED_BOOK_PASS

stop_server
printf '%s\n' "F04_SANDBOX_HTTP_PASS: loopback-only public cards/flag/revocations"
