#!/usr/bin/env bash
# Throwaway GitHub Actions-only VPS -> WireGuard -> home topology. No real hosts.
set -euo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true || -z "${RUNNER_TEMP:-}" ]]; then
  echo 'Refusing to run outside isolated GitHub Actions.' >&2
  exit 2
fi
if (( EUID != 0 )); then
  echo 'Root is required for throwaway network namespaces.' >&2
  exit 2
fi

CLIENT=tor-client-ci
EDGE=tor-edge-ci
HOME=tor-home-ci
REPORT_DIR="$RUNNER_TEMP/tor-vps-ingress"
mkdir -p "$REPORT_DIR"
chmod 755 "$REPORT_DIR"
KEYS="$(mktemp -d)"
chmod 700 "$KEYS"
edge_pid=''
home_pid=''
cleanup() {
  if [[ -n "$edge_pid" ]]; then kill "$edge_pid" 2>/dev/null || true; fi
  if [[ -n "$home_pid" ]]; then kill "$home_pid" 2>/dev/null || true; fi
  for namespace in "$CLIENT" "$EDGE" "$HOME"; do
    ip netns pids "$namespace" 2>/dev/null | xargs -r kill 2>/dev/null || true
    ip netns del "$namespace" 2>/dev/null || true
  done
  rm -rf "$KEYS"
}
trap cleanup EXIT

for namespace in "$CLIENT" "$EDGE" "$HOME"; do
  ip netns add "$namespace"
  ip -n "$namespace" link set lo up
done

# Client <-> public edge. No path from host runner into these namespaces.
ip link add edge-public type veth peer name client-public
ip link set edge-public netns "$EDGE"
ip link set client-public netns "$CLIENT"
ip -n "$EDGE" addr add 10.77.0.1/30 dev edge-public
ip -n "$CLIENT" addr add 10.77.0.2/30 dev client-public
ip -n "$EDGE" link set edge-public up
ip -n "$CLIENT" link set client-public up

# Edge <-> home: only encrypted WireGuard transport uses simulated WAN.
ip link add edge-wan type veth peer name home-wan
ip link set edge-wan netns "$EDGE"
ip link set home-wan netns "$HOME"
ip -n "$EDGE" addr add 10.78.0.1/30 dev edge-wan
ip -n "$HOME" addr add 10.78.0.2/30 dev home-wan
ip -n "$EDGE" link set edge-wan up
ip -n "$HOME" link set home-wan up

umask 077
wg genkey > "$KEYS/edge.key"
wg pubkey < "$KEYS/edge.key" > "$KEYS/edge.pub"
wg genkey > "$KEYS/home.key"
wg pubkey < "$KEYS/home.key" > "$KEYS/home.pub"
umask 022
ip -n "$EDGE" link add wg-edge type wireguard
ip -n "$HOME" link add wg-home type wireguard
ip netns exec "$EDGE" wg set wg-edge \
  private-key "$KEYS/edge.key" listen-port 51820 \
  peer "$(cat "$KEYS/home.pub")" allowed-ips 10.88.0.2/32 \
  endpoint 10.78.0.2:51820
ip netns exec "$HOME" wg set wg-home \
  private-key "$KEYS/home.key" listen-port 51820 \
  peer "$(cat "$KEYS/edge.pub")" allowed-ips 10.88.0.1/32 \
  endpoint 10.78.0.1:51820 persistent-keepalive 25
ip -n "$EDGE" addr add 10.88.0.1/30 dev wg-edge
ip -n "$HOME" addr add 10.88.0.2/30 dev wg-home
ip -n "$EDGE" link set wg-edge up
ip -n "$HOME" link set wg-home up
ip netns exec "$EDGE" ping -n -c 2 -W 3 10.88.0.2 > "$REPORT_DIR/wg-ping.txt"

cat > "$REPORT_DIR/nginx.conf" <<EOF
worker_processes 1;
pid $REPORT_DIR/nginx.pid;
error_log $REPORT_DIR/nginx.error.log warn;
events { worker_connections 2048; }
http {
  access_log off;
  sendfile on;
  keepalive_timeout 20;
  server {
    listen 10.77.0.1:18080;
    server_name _;
    location / {
      proxy_pass http://10.88.0.2:18081;
      proxy_http_version 1.1;
      proxy_set_header Connection "";
      proxy_set_header Host \$host;
      proxy_set_header X-Real-IP \$remote_addr;
      proxy_set_header X-Forwarded-For \$remote_addr;
      proxy_set_header X-Forwarded-Proto https;
      proxy_connect_timeout 3s;
      proxy_read_timeout 15s;
    }
  }
}
EOF

ip netns exec "$HOME" python3 tests/load/vps_ingress_backend.py \
  > "$REPORT_DIR/backend.log" 2>&1 &
home_pid=$!
ip netns exec "$EDGE" nginx -p "$REPORT_DIR/" -c "$REPORT_DIR/nginx.conf" \
  -g 'daemon off;' > "$REPORT_DIR/nginx.stdout" 2>&1 &
edge_pid=$!

for _ in $(seq 1 40); do
  if ip netns exec "$CLIENT" curl --noproxy '*' -fsS --max-time 2 \
      http://10.77.0.1:18080/4k -o /dev/null; then break; fi
  sleep 0.5
done
ip netns exec "$CLIENT" curl --noproxy '*' -fsS --max-time 3 \
  http://10.77.0.1:18080/4k -o /dev/null

# Spoofed forwarded headers must be replaced, not appended or trusted.
ip netns exec "$CLIENT" curl --noproxy '*' -fsS --max-time 3 \
  -H 'X-Real-IP: 203.0.113.77' -H 'X-Forwarded-For: 203.0.113.77' \
  http://10.77.0.1:18080/headers > "$REPORT_DIR/headers.json"
python3 - "$REPORT_DIR/headers.json" <<'PY'
import json
import sys
h = json.load(open(sys.argv[1], encoding="utf-8"))
assert h == {
    "real_ip": "10.77.0.2",
    "forwarded_for": "10.77.0.2",
    "forwarded_proto": "https",
}, h
PY

# 100 Mbit/s public port remains constant. Home upload varies by stage.
ip netns exec "$EDGE" tc qdisc replace dev edge-public root tbf \
  rate 100mbit burst 256kb latency 1000ms
printf 'home_up_mbit\tresponse\trequests_per_sec\ttransfer_per_sec\twrk_errors\n' \
  > "$REPORT_DIR/metrics.tsv"
for stage in '20 64k' '50 64k' '100 64k' '100 4k' '100 256k'; do
  read -r upload body <<< "$stage"
  ip netns exec "$HOME" tc qdisc replace dev home-wan root tbf \
    rate "${upload}mbit" burst 256kb latency 1000ms
  output="$REPORT_DIR/wrk-$upload-$body.txt"
  ip netns exec "$CLIENT" wrk -t 2 -c 16 -d 8s --timeout 10s --latency \
    "http://10.77.0.1:18080/$body" > "$output" 2>&1
  rps="$(awk '/Requests\/sec:/ {print $2}' "$output" | tail -n 1)"
  transfer="$(awk '/Transfer\/sec:/ {print $2}' "$output" | tail -n 1)"
  errors="$(grep -E 'Socket errors|Non-2xx or 3xx' "$output" | tr '\n' ';' || true)"
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$upload" "$body" "${rps:-?}" "${transfer:-?}" "${errors:-none}" \
    >> "$REPORT_DIR/metrics.tsv"
done

{
  printf '## Disposable edge -> WireGuard -> home CI (not a real VPS benchmark)\n\n'
  printf '| Home upload cap | Response | Measured req/s | Measured transfer/s | Errors |\n'
  printf '|---:|---|---:|---:|---|\n'
  awk -F '\t' 'NR>1 {printf "| %s Mbps | %s | %s | %s | %s |\n", $1,$2,$3,$4,$5}' \
    "$REPORT_DIR/metrics.tsv"
  printf '\nGitHub runner CPU, no TLS, no real TOR or PostgreSQL, no home ISP. '
  printf 'Use this only to validate tunnel/headers and network ceilings.\n'
} > "$REPORT_DIR/summary.md"

ip netns exec "$EDGE" wg show wg-edge > "$REPORT_DIR/wg-edge.txt"
ip netns exec "$HOME" wg show wg-home > "$REPORT_DIR/wg-home.txt"
ip netns exec "$HOME" tc -s qdisc show dev home-wan > "$REPORT_DIR/home-qdisc.txt"
ip netns exec "$EDGE" tc -s qdisc show dev edge-public > "$REPORT_DIR/edge-qdisc.txt"
printf 'Benchmark evidence: %s\n' "$REPORT_DIR"
cat "$REPORT_DIR/metrics.tsv"
