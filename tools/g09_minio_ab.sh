#!/usr/bin/env bash
# Dedicated disposable runner only. No external endpoint or credentials.
set -euo pipefail
arm="${1:?experiment arm}"
binary="${2:?MinIO executable}"
case "$arm" in release|parent|fixed) ;; *) exit 2 ;; esac
dir=$(mktemp -d "$RUNNER_TEMP/g09-ab-$arm.XXXXXX")
pid=
cleanup() {
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf -- "$dir"
}
trap cleanup EXIT
# Do not attach raw MinIO logs or packet data; only synthetic counters.
echo "G09_AB_ARM=$arm"
"$binary" --version
sha256sum "$binary"
MINIO_ROOT_USER=tor-g09-ab-ci MINIO_ROOT_PASSWORD=tor-g09-ab-synthetic-ci-only \
  "$binary" server "$dir" --address 127.0.0.1:9000 --console-address 127.0.0.1:9001 \
  > "$RUNNER_TEMP/g09-ab-$arm-minio.log" 2>&1 &
pid=$!
ready=0
for attempt in $(seq 1 60); do
  kill -0 "$pid" || exit 1
  if curl --fail --silent --max-time 2 http://127.0.0.1:9000/minio/health/ready >/dev/null; then
    ready=1
    break
  fi
  sleep 1
done
test "$ready" = 1
export TOR_TEST_S3_ENDPOINT=http://127.0.0.1:9000
timeout 600s cargo test -p tor-storage --features s3 --test g09_minio_ab -- \
  --ignored --nocapture --test-threads=1 2>&1 | tee "$RUNNER_TEMP/g09-ab-$arm-result.log"
# Both samples must be completed; a truncated run is not a clean result.
python3 - "$RUNNER_TEMP/g09-ab-$arm-result.log" <<'PY'
import re
import sys
from pathlib import Path

rows = re.findall(
    r"G09_AB_RESULT mode=(pooled|close) attempted=(\d+) first_get_ok=(\d+)"
    r" first_get_unavailable=(\d+) integrity_verified=(\d+)",
    Path(sys.argv[1]).read_text(),
)
assert sorted(row[0] for row in rows) == ["close", "pooled"], "missing or duplicate result"
for mode, attempted, ok, unavailable, integrity in rows:
    attempted, ok, unavailable, integrity = map(int, (attempted, ok, unavailable, integrity))
    assert attempted == integrity == 10000 and ok + unavailable == attempted, mode
PY
