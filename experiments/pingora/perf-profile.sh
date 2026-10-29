#!/usr/bin/env bash
# Optional hardware-counter / sampling diagnostic on a disposable CI runner only.
# Never used on the TOR VPS. Failure to access perf is an explicit NOT AVAILABLE.
set -Eeuo pipefail
cd "$(dirname "$0")/../.."
if [ "${GITHUB_ACTIONS:-}" != "true" ]; then
  echo "PERF_REFUSED: only disposable GitHub Actions workers" >&2
  exit 2
fi
command -v docker >/dev/null
command -v go >/dev/null
out="$PWD/experiments/pingora/perf-results"
root="$PWD/experiments/pingora"
mkdir -p "$out"
go build -o "$out/fixedrate" "$root/fixedrate.go"
container="tor-pingora-hotspots-$GITHUB_RUN_ID"
cleanup() {
  docker logs "$container" > "$out/server.log" 2>&1 || true
  docker rm -f "$container" >/dev/null 2>&1 || true
}
trap cleanup EXIT
{
  echo "kernel=$(uname -r)"
  echo "perf_event_paranoid=$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo unknown)"
  echo "scope=ephemeral_ci_not_production"
  echo "vm_limits=1_vcpu_1gib"
  echo "rate=1500_rps duration=12s path=/catalog"
  echo "note=CPU_counters_recorded_during_fixed_arrival_rate_not_at_maximum_throughput"
} > "$out/environment.txt"

if ! command -v perf >/dev/null; then
  echo "PERF_UNAVAILABLE: perf executable missing" | tee "$out/availability.txt"
  exit 0
fi
perf --version > "$out/perf-version.txt" 2>&1 || true

# Do not change the runner's security controls. Hardware events are optional.
hardware_events="cycles,instructions,task-clock,context-switches,cache-misses"
software_events="task-clock,context-switches"
# perf stat may return exit code 0 while hardware counters say '<not supported>'.
# A successful command alone is NOT evidence that cycles/instructions were counted.
if sudo perf stat -e "$hardware_events" -o "$out/probe.txt" -- sleep 0.15 > "$out/probe.log" 2>&1 \
    && ! grep -Eqi '<not supported>|<not counted>' "$out/probe.txt"; then
  events="$hardware_events"
  echo "PERF_AVAILABLE: all requested hardware and software events counted" | tee "$out/availability.txt"
elif sudo perf stat -e "$software_events" -o "$out/probe.txt" -- sleep 0.15 >> "$out/probe.log" 2>&1 \
    && ! grep -Eqi '<not supported>|<not counted>' "$out/probe.txt"; then
  events="$software_events"
  echo "PERF_PARTIAL: hardware counters unsupported; software task-clock and context-switches only" | tee "$out/availability.txt"
else
  echo "PERF_UNAVAILABLE: neither hardware nor software counters are readable" | tee "$out/availability.txt"
  exit 0
fi

docker run -d --name "$container" --cpus=1 --memory=1g --network=host \
  --volume "$PWD:/work:ro" --workdir /work ubuntu:24.04 \
  bash experiments/pingora/serve-benchmark.sh >/dev/null

ready=0
for _ in $(seq 1 90); do
  if curl -fsS --max-time 1 http://127.0.0.1:6193/catalog >/dev/null \
     && curl -fsS --max-time 1 http://127.0.0.1:6195/catalog >/dev/null; then
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
  echo "PERF_SERVICES_NOT_READY" >&2
  exit 1
fi

process_list="$(docker top "$container" -eo pid,comm)"
get_pids() {
  case "$1" in
    nginx) printf '%s\n' "$process_list" | awk 'NR>1 && $2=="nginx" {print $1}' | paste -sd, - ;;
    pingora) printf '%s\n' "$process_list" | awk 'NR>1 && $2 ~ /^tor-pingora/ {print $1}' | paste -sd, - ;;
  esac
}
for scheme in nginx pingora; do
  if [ "$scheme" = nginx ]; then port=6195; else port=6193; fi
  pids="$(get_pids "$scheme")"
  if [ -z "$pids" ]; then
    echo "PERF_MISSING_PROCESS: $scheme" >&2
    exit 1
  fi
  prefix="$out/$scheme"
  echo "SCHEME=$scheme PIDS=$pids EVENTS=$events" | tee -a "$out/environment.txt"
  sudo perf stat -e "$events" -p "$pids" \
    -o "$prefix.stat.txt" -- sleep 14 > "$prefix.stat.log" 2>&1 &
  monitor=$!
  sleep 1
  "$out/fixedrate" \
    -url "http://127.0.0.1:$port/catalog" -rate 1500 -seconds 12 \
    -output "$prefix.client.json" > "$prefix.client.log"
  if ! wait "$monitor"; then
    echo "PERF_STAT_FAILED scheme=$scheme" | tee -a "$out/availability.txt"
  fi
  if grep -Eqi '<not supported>|<not counted>' "$prefix.stat.txt"; then
    echo "PERF_STAT_PARTIAL scheme=$scheme: requested event not counted" | tee -a "$out/availability.txt"
  fi
  python3 - "$prefix.client.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["successful"] == 18000, data
assert data["dropped"] == 0, data
assert data["errors"] == 0, data
assert data["http_non200"] == 0, data
PY
  echo "PERF_LOAD_VALID scheme=$scheme rate=1500 errors=0 dropped=0"
done

# Sampling is separate from timing and throughput measurements. It adds overhead.
for scheme in nginx pingora; do
  if [ "$scheme" = nginx ]; then port=6195; else port=6193; fi
  pids="$(get_pids "$scheme")"
  prefix="$out/$scheme"
  if sudo perf record -e task-clock -F 199 -g --call-graph dwarf,8192 -p "$pids" \
    -o "$prefix.perf.data" -- sleep 26 > "$prefix.record.log" 2>&1 &
  then
    sampler=$!
  else
    echo "PERF_RECORD_COULD_NOT_START scheme=$scheme" | tee -a "$out/availability.txt"
    continue
  fi
  sleep 1
  "$out/fixedrate" \
    -url "http://127.0.0.1:$port/catalog" -rate 1500 -seconds 24 \
    -output "$prefix.sampling.client.json" > "$prefix.sampling.client.log"
  if wait "$sampler"; then
    if ! sudo perf report --stdio --no-children --sort comm,dso,symbol \
      -i "$prefix.perf.data" > "$prefix.top-symbols.txt" 2> "$prefix.report.err"; then
      echo "PERF_REPORT_FAILED scheme=$scheme" | tee -a "$out/availability.txt"
    fi
    # The textual report is the portable evidence; discard raw profile if large.
    bytes=$(stat -c '%s' "$prefix.perf.data")
    if [ "$bytes" -gt 10485760 ]; then
      echo "PERF_RAW_TOO_LARGE scheme=$scheme bytes=$bytes" >> "$out/availability.txt"
      rm -f "$prefix.perf.data"
    fi
  else
    echo "PERF_RECORD_FAILED scheme=$scheme" | tee -a "$out/availability.txt"
  fi
done
echo "PERF_PROBE_COMPLETED: review availability.txt and top-symbols.txt"