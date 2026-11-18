#!/usr/bin/env bash
# DROP-08b: real PG18 planner/query behavior on disposable, session-local tables.
# NOT external DR, actual S3 readback, key recovery or production approval.
set -euo pipefail
expected='postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci'
if [[ "${MIGRATOR_URL:-}" != "$expected" ]]; then
  echo "DROP08_PG_FIXTURE_REFUSED: disposable local CI database required" >&2
  exit 2
fi
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

for scenario in happy wrong_layer_book paid_layer_chapter wrong_single_sha wrong_pack_sha candidate_only; do
  # No persistent book or entitlement rows are changed: all fixtures use pg_temp.
  psql -Xq --no-password --dbname "$MIGRATOR_URL" \
    -v ON_ERROR_STOP=1 -v "scenario=$scenario" \
    -f tools/drop08_pg_fixture.sql > "$tmp/export.csv"
  if [[ "$scenario" == happy ]]; then
    python3 tools/drop08_verify_historical_free_pins.py < "$tmp/export.csv" > "$tmp/result" 2>&1
    grep -q 'TOR_DROP08_HISTORICAL_PINS_METADATA_OK layer=1 saved=1' "$tmp/result"
  else
    set +e
    python3 tools/drop08_verify_historical_free_pins.py < "$tmp/export.csv" > "$tmp/result" 2>&1
    exit_code=$?
    set -e
    [[ "$exit_code" -eq 2 ]] || { echo "DROP08_PG_FIXTURE_REFUSED: expected verifier failure" >&2; exit 1; }
    if [[ "$scenario" == wrong_layer_book || "$scenario" == paid_layer_chapter ]]; then
      grep -q 'invalid historical chapter-version or Layer book membership' "$tmp/result"
    else
      grep -q 'missing_source_receipts=1' "$tmp/result"
    fi
  fi
done
echo "DROP08_PG18_FIXTURES_OK"
