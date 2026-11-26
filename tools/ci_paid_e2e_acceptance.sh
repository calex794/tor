#!/usr/bin/env bash
# CI-only disposable PostgreSQL and private loopback S3 fixtures.
set -euo pipefail
if [[ "${GITHUB_ACTIONS:-}" != true || "${PGHOST:-}" != 127.0.0.1 ||
      "${PGDATABASE:-}" != postgres ||
      "${TEST_DATABASE_URL:-}" != postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci ||
      "${MIGRATOR_URL:-}" != postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci ]]; then
  echo 'Refusing non-isolated paid E2E fixture' >&2
  exit 2
fi
if [[ "${TOR_TEST_S3_ENDPOINT:-}" != http://127.0.0.1:9000 ]]; then
  echo 'Refusing non-loopback paid E2E S3 endpoint' >&2
  exit 2
fi
set -euo pipefail
export TOR_PAID_GRANT_FIXTURE_PATH="$RUNNER_TEMP/paid-grant-e2e.json"
foundation_log="$(mktemp "$RUNNER_TEMP/tor-fnd07.XXXXXXXX")"
trap 'rm -f "$TOR_PAID_GRANT_FIXTURE_PATH" "$foundation_log"' EXIT
python3 tools/check_feature_layout.py
cargo fmt -p tor-web -- --check
node --check tools/paid_grant_webcrypto_e2e.mjs
node --check tools/layer_reader_minio_chromium_e2e.cjs
node --check tools/paid_reader_minio_chromium_e2e.cjs
node --check tools/s035_contiguous_paid_browser.cjs
node --check tools/paid_reader_private_media_browser.cjs
node --check tools/drop_06b_pack_chromium.cjs
node --check tools/d05_layer_toc_chromium.cjs
node --check tools/d05_layer_free_live_chromium.cjs
node --check tools/d05_four_live_layer_chromium.cjs
node --check tools/d05_four_archived_layer_live_chromium.cjs
node tools/drop_06b_pack_chromium.cjs
# True Chromium validates 2 archived FREE + 2 Layer PAID TOC
# without public /reader links, escaped titles, or full PACK URLs.
node tools/d05_layer_toc_chromium.cjs
cargo test -p tor-web --features s3-paid --test g09_operator_http_pg -- --ignored --test-threads=1
TOR_PAID_CHECKOUT_ENABLED=1 TOR_LAYER_READER_ENABLED=1 cargo test -p tor-web --features s3-paid --test paid_grants_s3_pg --features s3-free -- --ignored --test-threads=1
node tools/layer_reader_minio_chromium_e2e.cjs
# Keep the 30s S3 URL checks next to their fixture, never after slow Cargo tests.
node tools/paid_grant_webcrypto_e2e.mjs
node tools/paid_reader_minio_chromium_e2e.cjs
node tools/paid_reader_private_media_browser.cjs
cargo test -p tor-web --features s3-paid --test paid_author_pg -- --ignored --test-threads=1
cargo test -p tor-web --features s3-paid --test paid_scheduled_author_pg -- --ignored --test-threads=1
# free_author_s3_pg includes the DROP-04b/06b PG18+MinIO cases;
# do not rerun the same fixed-identity fixtures in this database.
TOR_FREE_S3_PUBLIC_ORIGIN=http://127.0.0.1:9000 cargo test -p tor-web --features s3-free --test free_author_s3_pg -- --ignored --test-threads=1 --show-output
# D-05/#1022: a real old Layer buyer opens immutable FREE V1
# from private SINGLE after owner deletion and moderator approval.
TOR_FREE_S3_PUBLIC_ORIGIN=http://127.0.0.1:9000 cargo test -p tor-web --features s3-free,s3-paid --test layer_free_private_s3_pg -- --ignored --test-threads=1 --show-output
# DROP-07a: actual-byte evidence only, on this disposable PG/MinIO.
# These checks cannot authorize orphan classification or S3 DELETE.
cargo test -p tor-books --test free_expected_inventory_pg -- --ignored --test-threads=1 --show-output
cargo test -p tor-books --test free_inventory_cycles_pg -- --ignored --test-threads=1 --show-output
cargo test -p tor-web --features s3-free --test free_inventory_operator_pg_minio -- --ignored --test-threads=1 --show-output | tee "$foundation_log"
cargo test -p tor-storage --features s3 --test free_expected_inventory_s3 -- --ignored --test-threads=1 --show-output
bash tools/drop_04d_worker_ci.sh | tee -a "$foundation_log"
python3 tools/verify_ci_gate.py record-foundation paid-private-e2e "$foundation_log"
