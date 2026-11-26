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
set -euo pipefail
release=minio.linux-amd64.RELEASE.2025-09-07T16-13-09Z
url=https://github.com/minio/minio/releases/download/RELEASE.2025-09-07T16-13-09Z
curl --fail --location --silent --show-error --retry 3 \
  "$url/$release" --output "$RUNNER_TEMP/$release"
curl --fail --location --silent --show-error --retry 3 \
  "$url/$release.sha256sum" --output "$RUNNER_TEMP/$release.sha256sum"
expected=$(grep -Eo '[0-9a-f]{64}' "$RUNNER_TEMP/$release.sha256sum" | head -n 1)
test "$(printf '%s' "$expected" | wc -c)" -eq 64
actual=$(sha256sum "$RUNNER_TEMP/$release" | cut -d ' ' -f1)
test "$actual" = "$expected"
chmod 700 "$RUNNER_TEMP/$release"
mkdir -p "$RUNNER_TEMP/tor-paid-minio"
MINIO_API_CORS_ALLOW_ORIGIN=http://127.0.0.1:3098 \
MINIO_ROOT_USER=tor-chapter-ci-user MINIO_ROOT_PASSWORD=tor-chapter-ci-secret-only \
  nohup "$RUNNER_TEMP/$release" server "$RUNNER_TEMP/tor-paid-minio" \
    --address 127.0.0.1:9000 --console-address 127.0.0.1:9001 \
    > "$RUNNER_TEMP/tor-paid-minio.log" 2>&1 &
ready=0
for attempt in $(seq 1 60); do
  if curl --fail --silent http://127.0.0.1:9000/minio/health/ready > /dev/null; then
    ready=1
    break
  fi
  sleep 1
done
if [ "$ready" != 1 ]; then
  tail -n 80 "$RUNNER_TEMP/tor-paid-minio.log"
  exit 1
fi
