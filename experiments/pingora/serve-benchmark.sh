#!/usr/bin/env bash
# This script runs *inside* a one-core/1-GiB disposable Docker container.
set -Eeuo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/tmp/pingora-apt.log 2>&1
apt-get install -y -qq nginx libssl3t64 >>/tmp/pingora-apt.log 2>&1 || {
  cat /tmp/pingora-apt.log
  exit 1
}
nginx -t -c /work/experiments/pingora/nginx-benchmark.conf

/work/experiments/pingora/target/debug/examples/axum_fixture &
fixture_pid=$!
PINGORA_EXPERIMENT=1 \
PINGORA_EXPERIMENT_BIND=127.0.0.1:6193 \
PINGORA_EXPERIMENT_UPSTREAM=127.0.0.1:6194 \
  /work/experiments/pingora/target/debug/tor-pingora-experiment &
pingora_pid=$!
nginx -c /work/experiments/pingora/nginx-benchmark.conf -g 'daemon off;' &
nginx_pid=$!

trap 'kill "$fixture_pid" "$pingora_pid" "$nginx_pid" 2>/dev/null || true' EXIT
wait -n "$fixture_pid" "$pingora_pid" "$nginx_pid"
echo 'BENCHMARK_PROCESS_EXITED: a required HTTP service stopped' >&2
exit 1
