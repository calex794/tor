#!/usr/bin/env bash
set -Eeuo pipefail
cd "$(dirname "$0")/../.."
test "${GITHUB_ACTIONS:-}" = true
command -v docker >/dev/null
command -v go >/dev/null
root="$PWD/experiments/pingora"
out="$root/fixed-results"
mkdir -p "$out"
go build -o "$out/fixedrate" "$root/fixedrate.go"
name="tor-pingora-fixed-$GITHUB_RUN_ID"
cleanup() { docker logs "$name" > "$out/server.log" 2>&1 || true; docker rm -f "$name" >/dev/null 2>&1 || true; }
trap cleanup EXIT
for cpus in 1 2; do
 docker run -d --name "$name" --cpus="$cpus" --memory=1g --network=host -v "$PWD:/work:ro" -w /work ubuntu:24.04 bash experiments/pingora/serve-benchmark.sh >/dev/null
 ready=0
 for i in $(seq 1 90); do
  if curl -fsS --max-time 1 http://127.0.0.1:6193/catalog >/dev/null && curl -fsS --max-time 1 http://127.0.0.1:6195/catalog >/dev/null; then ready=1;break;fi
  sleep 1
 done
 if [ "$ready" != 1 ];then docker logs "$name";exit 1;fi
 for rate in 500 1500 3000; do
  for round in 1 2;do
   if [ "$round" = 1 ];then order="nginx pingora";else order="pingora nginx";fi
   for scheme in $order;do
    if [ "$scheme" = nginx ];then port=6195;else port=6193;fi
    prefix="$out/cpu${cpus}-r${rate}-round${round}-$scheme"
    before=$(curl -fsS http://127.0.0.1:6194/__bench/accepts)
    python3 "$root/process_snapshot.py" "$name" > "$prefix.before.json"
    "$out/fixedrate" -url "http://127.0.0.1:$port/catalog" -rate "$rate" -seconds 12 -output "$prefix.client.json"
    python3 "$root/process_snapshot.py" "$name" > "$prefix.after.json"
    after=$(curl -fsS http://127.0.0.1:6194/__bench/accepts)
    python3 "$root/process_report.py" "$prefix" "$before" "$after" "$cpus" "$rate" "$round" "$scheme"
   done
  done
 done
 docker rm -f "$name" >/dev/null
done
python3 "$root/summarize_fixed.py" "$out"
echo FIXED_RATE_PROFILE_COMPLETE
