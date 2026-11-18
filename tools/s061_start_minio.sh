#!/usr/bin/env bash
# Manual S061 drill only. No access to an externally supplied bucket or host.
set -euo pipefail
test "${GITHUB_ACTIONS:-}" = true
: "${RUNNER_TEMP:?requires disposable runner directory}"
release=minio.linux-amd64.RELEASE.2025-09-07T16-13-09Z
url=https://github.com/minio/minio/releases/download/RELEASE.2025-09-07T16-13-09Z
bin="$RUNNER_TEMP/s061-$release"
curl -fsSL --retry 3 "$url/$release" --output "$bin"
curl -fsSL --retry 3 "$url/$release.sha256sum" --output "$bin.sha256sum"
read -r expected _ < "$bin.sha256sum"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]]
printf '%s  %s\n' "$expected" "$bin" | sha256sum --check --status
chmod 700 "$bin"
mkdir "$RUNNER_TEMP/s061-minio"
MINIO_API_CORS_ALLOW_ORIGIN=http://127.0.0.1:3091,http://127.0.0.1:3092 \
MINIO_ROOT_USER=tor-s061-ci-user MINIO_ROOT_PASSWORD=tor-s061-ci-secret-only \
  nohup "$bin" server "$RUNNER_TEMP/s061-minio" --address 127.0.0.1:9012 \
    --console-address 127.0.0.1:9013 > "$RUNNER_TEMP/s061-minio.log" 2>&1 &
pid=$!
for attempt in $(seq 1 45); do
  if curl -fsS --max-time 2 http://127.0.0.1:9012/minio/health/ready > /dev/null 2>&1; then
    kill -0 "$pid"
    exit 0
  fi
  kill -0 "$pid" || break
  sleep 1
done
tail -n 60 "$RUNNER_TEMP/s061-minio.log"
exit 1
