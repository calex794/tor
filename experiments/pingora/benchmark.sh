#!/usr/bin/env bash
# All services run inside one Docker container constrained to 1 vCPU and 1 GiB.
# wrk runs on the *host*, so its CPU usage does not steal the server quota.
set -Eeuo pipefail
cd "$(dirname "$0")/../.."
if [ "$(printenv GITHUB_ACTIONS || true)" != "true" ]; then
  echo 'Refusing the benchmark outside disposable GitHub Actions' >&2
  exit 2
fi
command -v docker >/dev/null
command -v wrk >/dev/null
root="$PWD/experiments/pingora"
results="$root/results"
mkdir -p "$results"
container="tor-pingora-bench-$GITHUB_RUN_ID"
cleanup() {
  docker logs "$container" > "$results/server.log" 2>&1 || true
  docker rm -f "$container" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run -d --name "$container" --cpus=1 --memory=1g \
  --network=host --volume "$PWD:/work:ro" --workdir /work \
  ubuntu:24.04 bash experiments/pingora/serve-benchmark.sh
echo "BENCHMARK_CONTAINER_LIMITS $(docker inspect -f 'cpu_nanocpus={{.HostConfig.NanoCpus}} memory_bytes={{.HostConfig.Memory}}' "$container")"

ready=0
for _ in $(seq 1 90); do
  if curl -fsS --max-time 1 http://127.0.0.1:6195/catalog >/dev/null \
     && curl -fsS --max-time 1 http://127.0.0.1:6193/catalog >/dev/null \
     && curl -fsS --max-time 1 http://127.0.0.1:6196/catalog >/dev/null; then
    ready=1
    break
  fi
  if [ "$(docker inspect -f '{{.State.Running}}' "$container")" != true ]; then
    break
  fi
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  docker logs "$container"
  echo 'BENCHMARK_SERVICES_NOT_READY' >&2
  exit 1
fi

# Check functional equivalence, including errors, before measuring.
for port in 6195 6193 6196; do
  for path in / /catalog /books/1 /api/v1/books; do
    actual=$(curl --silent --show-error --max-time 5 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port$path")
    test "$actual" = 200 || { echo "UNEXPECTED_HTTP $port $path $actual"; exit 1; }
  done
  for pair in '/missing:404' '/status/429:429' '/status/503:503'; do
    path=$(printf '%s' "$pair" | cut -d: -f1)
    expected=$(printf '%s' "$pair" | cut -d: -f2)
    actual=$(curl --silent --show-error --max-time 5 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port$path")
    test "$actual" = "$expected" || { echo "UNEXPECTED_HTTP $port $path $actual"; exit 1; }
  done
done
echo 'BENCHMARK_HTTP_EQUIVALENCE_PASS schemes=3 statuses=200,404,429,503'

port_for() {
  case "$1" in
    nginx) echo 6195 ;;
    pingora) echo 6193 ;;
    nginx_pingora) echo 6196 ;;
    *) exit 2 ;;
  esac
}

# Warm up every measured endpoint through each proxy.
for route in catalog books/1 api/v1/books; do
  for scheme in nginx pingora nginx_pingora; do
    port=$(port_for "$scheme")
    wrk -t1 -c16 -d2s -s "$root/wrk_metrics.lua" "http://127.0.0.1:$port/$route" >/dev/null
  done
done

echo 'round route scheme TOR_PINGORA_METRIC fields' > "$results/measurements.txt"
echo 'round route scheme docker_stats' > "$results/resource-samples.txt"
for round in 1 2 3 4 5; do
  case "$round" in
    1|4) order='nginx pingora nginx_pingora' ;;
    2|5) order='pingora nginx_pingora nginx' ;;
    3) order='nginx_pingora nginx pingora' ;;
  esac
  for route in catalog books/1 api/v1/books; do
    for scheme in $order; do
      port=$(port_for "$scheme")
      filename="$results/run-$round-$scheme-$(echo "$route" | tr / _).log"
      wrk -t1 -c16 -d10s --latency -s "$root/wrk_metrics.lua" \
        "http://127.0.0.1:$port/$route" > "$filename"
      metric=$(grep '^TOR_PINGORA_METRIC ' "$filename" || true)
      if [ -z "$metric" ]; then
        echo "MISSING_WRK_METRIC $filename" >&2
        exit 1
      fi
      printf 'round=%s route=%s scheme=%s %s\n' "$round" "$route" "$scheme" "$metric" | tee -a "$results/measurements.txt"
      printf 'round=%s route=%s scheme=%s %s\n' "$round" "$route" "$scheme" \
        "$(docker stats --no-stream --format 'cpu={{.CPUPerc}} memory={{.MemUsage}}' "$container")" \
        >> "$results/resource-samples.txt"
    done
  done
done
echo 'BENCHMARK_DONE repeats=5 routes=3 schemes=3 duration=10s_each'
