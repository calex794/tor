#!/usr/bin/env bash
# Disposable 100k dense public catalogue; never contact external hosts.
set -euo pipefail
[[ "$GITHUB_EVENT_NAME" == pull_request && "$PGHOST" == 127.0.0.1 &&
   "$TEST_DATABASE_URL" == */tor_ci && "$MIGRATOR_URL" == */tor_ci ]] ||
  { echo 'Refusing non-PR/non-disposable database' >&2; exit 1; }
identity="$(psql -X -A -t -q "$TEST_DATABASE_URL" -c 'SELECT current_database(),current_user')"
[[ "$identity" == 'tor_ci|tor_app' ]] ||
  { echo 'Wrong database/role; seed refused' >&2; exit 1; }
out="$RUNNER_TEMP/tor-load/dense100k"
mkdir -p "$out"
psql -X -v ON_ERROR_STOP=1 "$TEST_DATABASE_URL" <<'SQL'
UPDATE books SET genres=ARRAY['ci-dense-genre'],
  tags=ARRAY['ci-dense-tag'],fandoms=ARRAY['ci-dense-fandom']
WHERE id BETWEEN 1 AND 10000;
INSERT INTO books(id,source_identity,target_language,title,visibility,genres,tags,fandoms)
SELECT n,'ci:load:book:' || n,'ru','Load Test Book ' || n,'public',
       ARRAY['ci-dense-genre'],ARRAY['ci-dense-tag'],ARRAY['ci-dense-fandom']
FROM generate_series(10001,100000) n;
SQL
psql -X -v ON_ERROR_STOP=1 "$MIGRATOR_URL" -c 'ANALYZE books'
psql -X -A -t "$TEST_DATABASE_URL" -c \
 "SELECT COUNT(*), COUNT(*) FILTER(WHERE cardinality(genres)>0) FROM books" \
 > "$out/fixture-counts.txt"

server_pid=""
monitors=()
cleanup() {
  for pid in "${monitors[@]}"; do kill "$pid" 2>/dev/null || true; done
  if [[ -n "$server_pid" ]]; then kill "$server_pid" 2>/dev/null || true; fi
}
trap cleanup EXIT
DATABASE_URL="$TEST_DATABASE_URL" TOR_LISTEN_ADDR=127.0.0.1:3089 \
 TOR_AUTH_ENABLED=0 TOR_PAID_CHECKOUT_ENABLED=0 \
 ./target/release/tor-web > "$out/server.log" 2>&1 &
server_pid="$!"
ready=0
for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:3089/readyz >/dev/null 2>&1; then ready=1; break; fi
  if ! kill -0 "$server_pid" 2>/dev/null; then break; fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then tail -n 100 "$out/server.log"; exit 1; fi
curl --fail --silent --show-error http://127.0.0.1:3089/catalog -o /dev/null

pidstat -h -u -r -d -C 'tor-web|k6|postgres' 1 > "$out/pidstat.txt" 2>&1 &
monitors+=("$!")
vmstat 1 > "$out/vmstat.txt" 2>&1 &
monitors+=("$!")
(
  while kill -0 "$server_pid" 2>/dev/null; do
    printf '%s\t' "$(date -u +%FT%TZ)"
    psql -X -A -t -q "$TEST_DATABASE_URL" -c "
      SELECT coalesce(state,'unknown') || '/' ||
             coalesce(wait_event_type,'cpu-or-none') || '/' ||
             coalesce(wait_event,'none') || ':' || count(*)
      FROM pg_stat_activity
      WHERE datname='tor_ci' AND usename='tor_app'
      GROUP BY state,wait_event_type,wait_event
      ORDER BY state,wait_event_type,wait_event" | tr '\n' ';'
    printf '\n'
    sleep 2
  done
) > "$out/pg-waits.tsv" 2>&1 &
monitors+=("$!")

printf 'profile\trps\tduration\toutcome\n' > "$out/results.tsv"
successes=0
run_profile() {
  local profile="$1" rate="$2" seconds="$3" outcome
  echo "::group::dense100k $profile target=$rate RPS duration=$seconds s"
  if TOR_LOAD_PROFILE="$profile" TARGET_RPS="$rate" STAGE_SECONDS="$seconds" \
    k6 run --summary-export "$out/summary-$profile-$rate-$seconds-s.json" \
      tests/load/tor_http.js 2>&1 | tee "$out/k6-$profile-$rate-$seconds-s.log"; then
    outcome=PASS
    successes=$((successes + 1))
  else
    outcome=FAIL
  fi
  printf '%s\t%s\t%s\t%s\n' "$profile" "$rate" "$seconds" "$outcome" >> "$out/results.tsv"
  echo "::endgroup::"
}
for rate in 25 50 100 200 250; do run_profile catalog "$rate" 15; done
run_profile catalog_cursor 250 15
run_profile catalog 50 60
{
  echo '## PERF-233: 100k dense catalogue, isolated GitHub runner'
  echo '| Profile | Target RPS | Seconds | Result |'
  echo '|---|---:|---:|---|'
  awk -F '\t' 'NR>1 {printf "| %s | %s | %s | %s |\n",$1,$2,$3,$4}' "$out/results.tsv"
  echo
  echo 'FAIL marks a measured capacity limit; higher stages are diagnostic, not release gates.'
  echo 'Traffic is 127.0.0.1 only. The real VPS is not contacted.'
} >> "$GITHUB_STEP_SUMMARY"
cat "$out/results.tsv"
if (( successes == 0 )); then echo 'No stage passed' >&2; exit 1; fi
