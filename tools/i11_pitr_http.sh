#!/usr/bin/env bash
# Only invoked inside the mount-free, network=none synthetic PG18 PITR sandbox.
set -Eeuo pipefail
umask 077
[[ $# == 1 && $1 =~ ^/tmp/tor-i11-pitr\.[A-Za-z0-9]{8}$ ]] || exit 2
[[ ${TOR_I11_DISPOSABLE_CONTAINER:-} == yes && -f /.dockerenv && $(id -u) != 0 ]] || exit 2
work=$1
export TOR_DR_S3_ENDPOINT=http://127.0.0.1:9010
export TOR_DR_S3_ACCESS_KEY=tor-dr-ci-user
export TOR_DR_S3_SECRET_KEY=tor-dr-ci-secret-only
export GITHUB_ACTIONS=true
export TOR_LAYER_FREE_ARCHIVE_ENABLED=1
export DROP07_RESTORED_DATABASE_URL=postgresql://tor_app@127.0.0.1:15436/tor_ci
export DROP07_RIGHTS_FILE="$work/rights.json"
[[ $(psql "$DROP07_RESTORED_DATABASE_URL" -XAtq -v ON_ERROR_STOP=1 -c "SELECT current_user='tor_app' AND NOT pg_is_in_recovery()") == t ]]
bucket="$TOR_DR_S3_ENDPOINT/tor-dr-ci-book-pack"
stem=private/free-packs/import/ci/drop07-restore-v1
mkdir -p "$work/minio"
HOME="$work" MINIO_ROOT_USER="$TOR_DR_S3_ACCESS_KEY" MINIO_ROOT_PASSWORD="$TOR_DR_S3_SECRET_KEY" \
  MINIO_BROWSER=off MINIO_UPDATE=off /opt/tor-i11-bin/minio server "$work/minio" \
  --address 127.0.0.1:9010 --console-address 127.0.0.1:9011 >"$work/log/http-minio.log" 2>&1 &
minio_pid=$!
cleanup() {
  kill "$minio_pid" 2>/dev/null || true
  wait "$minio_pid" 2>/dev/null || true
}
trap cleanup EXIT
ready=false
for attempt in $(seq 1 40); do
  if curl --noproxy '*' -fsS --max-time 2 "$TOR_DR_S3_ENDPOINT/minio/health/ready" >/dev/null 2>&1; then
    ready=true
    break
  fi
  kill -0 "$minio_pid" 2>/dev/null || exit 1
  sleep 0.5
done
[[ $ready == true ]]
s3() {
  curl --noproxy '*' --silent --show-error --fail --max-time 10 \
    --aws-sigv4 'aws:amz:us-east-1:s3' \
    --user "$TOR_DR_S3_ACCESS_KEY:$TOR_DR_S3_SECRET_KEY" "$@"
}
s3 --request PUT --output /dev/null "$bucket"
s3 --upload-file "$work/pack.zst" --output /dev/null "$bucket/$stem.zst"
s3 --upload-file "$work/pack.manifest.json" --output /dev/null "$bucket/$stem.manifest.json"
[[ $(curl --noproxy '*' -sS --max-time 5 -o /dev/null -w '%{http_code}' "$bucket/$stem.zst") == 403 ]]
# The real router verifies exact V1 plaintext/hash/cache headers and the
# complete saved/Layer denial matrix over loopback TCP, not mocked responses.
if ! /opt/tor-i11-bin/http-reader-test verify_physical_free_archive \
    --exact --ignored --test-threads=1 --show-output >"$work/log/physical-http.log" 2>&1; then
  echo 'I11_PITR_PHYSICAL_HTTP_TEST_FAILED' >&2
  exit 1
fi
grep -Fxq 'FND05_PHYSICAL_FREE_LAYER_HTTP_OK' "$work/log/physical-http.log"
