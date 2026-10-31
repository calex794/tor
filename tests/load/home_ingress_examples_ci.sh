#!/usr/bin/env bash
# Validate the actual future Nginx examples in private GitHub Actions namespaces.
# No real hosts, DNS, certs or servers are accessed.
set -euo pipefail
if [[ "${GITHUB_ACTIONS:-}" != true || -z "${RUNNER_TEMP:-}" || "$EUID" != 0 ]]; then
  echo 'This test is only for root-owned disposable GitHub Actions.' >&2
  exit 2
fi
EDGE=tor-ex-edge
HOME=tor-ex-home
CLIENT=tor-ex-client
REPORT="$RUNNER_TEMP/tor-vps-ingress/examples"
TMP="$(mktemp -d "$RUNNER_TEMP/tor-examples.XXXXXX")"
chmod 700 "$TMP"
mkdir -p "$REPORT"
edge_pid=''
home_pid=''
backend_pid=''
cleanup() {
  for pid in "$edge_pid" "$home_pid" "$backend_pid"; do
    if [[ -n "$pid" ]]; then kill "$pid" 2>/dev/null || true; fi
  done
  for ns in "$CLIENT" "$EDGE" "$HOME"; do
    ip netns pids "$ns" 2>/dev/null | xargs -r kill 2>/dev/null || true
    ip netns del "$ns" 2>/dev/null || true
  done
  rm -rf "$TMP"
}
trap cleanup EXIT

umask 077
openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 1 \
  -keyout "$TMP/tls.key" -out "$TMP/tls.crt" \
  -subj '/CN=pandaolate.ru' > /dev/null 2>&1

# Rewrite public listening ports and cert paths ONLY, preserving production
# example routing, access controls and headers for this private lab.
python3 - "$TMP" <<'PY'
from pathlib import Path
import sys

tmp = Path(sys.argv[1])
source = Path("ops/home-ingress")
edge = (source / "vps-site.nginx.conf.example").read_text(encoding="utf-8")
for old, new in [
    ("listen 80 default_server;", "listen 10.77.0.1:18080 default_server;"),
    ("listen 80;", "listen 10.77.0.1:18080;"),
    ("listen 443 ssl default_server;", "listen 10.77.0.1:18443 ssl default_server;"),
    ("listen 443 ssl;", "listen 10.77.0.1:18443 ssl;"),
    ("/etc/letsencrypt/live/pandaolate.ru/fullchain.pem", str(tmp / "tls.crt")),
    ("/etc/letsencrypt/live/pandaolate.ru/privkey.pem", str(tmp / "tls.key")),
    ("/var/www/acme", str(tmp)),
]:
    if old not in edge:
        raise SystemExit(f"missing expected Nginx setting: {old}")
    edge = edge.replace(old, new)
(tmp / "edge-site.conf").write_text(edge, encoding="utf-8")
home = (source / "home-site.nginx.conf.example").read_text(encoding="utf-8")
(tmp / "home-site.conf").write_text(home, encoding="utf-8")
for role in ("edge", "home"):
    config = (
        "worker_processes 1;\n"
        f"pid {tmp}/{role}.pid;\n"
        f"error_log {tmp}/{role}.error.log info;\n"
        "events { worker_connections 128; }\n"
        f"http {{ include {tmp}/{role}-site.conf; }}\n"
    )
    (tmp / f"{role}.nginx.conf").write_text(config, encoding="utf-8")
PY

for ns in "$CLIENT" "$EDGE" "$HOME"; do
  ip netns add "$ns"
  ip -n "$ns" link set lo up
done

ip link add tor-ex-e-pub type veth peer name tor-ex-c-pub
ip link set tor-ex-e-pub netns "$EDGE"
ip link set tor-ex-c-pub netns "$CLIENT"
ip -n "$EDGE" addr add 10.77.0.1/30 dev tor-ex-e-pub
ip -n "$CLIENT" addr add 10.77.0.2/30 dev tor-ex-c-pub
ip -n "$EDGE" link set tor-ex-e-pub up
ip -n "$CLIENT" link set tor-ex-c-pub up

ip link add tor-ex-e-wan type veth peer name tor-ex-h-wan
ip link set tor-ex-e-wan netns "$EDGE"
ip link set tor-ex-h-wan netns "$HOME"
ip -n "$EDGE" addr add 10.78.0.1/30 dev tor-ex-e-wan
ip -n "$HOME" addr add 10.78.0.2/30 dev tor-ex-h-wan
ip -n "$EDGE" link set tor-ex-e-wan up
ip -n "$HOME" link set tor-ex-h-wan up

wg genkey > "$TMP/edge.key"
wg pubkey < "$TMP/edge.key" > "$TMP/edge.pub"
wg genkey > "$TMP/home.key"
wg pubkey < "$TMP/home.key" > "$TMP/home.pub"
ip -n "$EDGE" link add wg-edge type wireguard
ip -n "$HOME" link add wg-home type wireguard
ip netns exec "$EDGE" wg set wg-edge \
  private-key "$TMP/edge.key" listen-port 51820 \
  peer "$(cat "$TMP/home.pub")" allowed-ips 10.88.0.2/32
ip netns exec "$HOME" wg set wg-home \
  private-key "$TMP/home.key" listen-port 51820 \
  peer "$(cat "$TMP/edge.pub")" allowed-ips 10.88.0.1/32 \
  endpoint 10.78.0.1:51820 persistent-keepalive 25
ip -n "$EDGE" addr add 10.88.0.1/30 dev wg-edge
ip -n "$HOME" addr add 10.88.0.2/30 dev wg-home
ip -n "$EDGE" link set wg-edge up
ip -n "$HOME" link set wg-home up
ip netns exec "$HOME" ping -n -c 2 -W 3 10.88.0.1 > "$REPORT/wg-init.txt"

ip netns exec "$EDGE" nginx -t -p "$TMP/" -c "$TMP/edge.nginx.conf" \
  > "$REPORT/edge-nginx-test.txt" 2>&1
ip netns exec "$HOME" nginx -t -p "$TMP/" -c "$TMP/home.nginx.conf" \
  > "$REPORT/home-nginx-test.txt" 2>&1

# The mock TOR binds the loopback interface, exactly like real Rust TOR.
TOR_TEST_LISTEN_HOST=127.0.0.1 TOR_TEST_LISTEN_PORT=3000 \
  ip netns exec "$HOME" python3 tests/load/vps_ingress_backend.py \
  > "$REPORT/backend.log" 2>&1 &
backend_pid=$!
ip netns exec "$HOME" nginx -p "$TMP/" -c "$TMP/home.nginx.conf" \
  -g 'daemon off;' > "$REPORT/home-nginx.log" 2>&1 &
home_pid=$!
ip netns exec "$EDGE" nginx -p "$TMP/" -c "$TMP/edge.nginx.conf" \
  -g 'daemon off;' > "$REPORT/edge-nginx.log" 2>&1 &
edge_pid=$!

probe() {
  ip netns exec "$CLIENT" curl --noproxy '*' \
    --resolve "pandaolate.ru:18443:10.77.0.1" -kfsS --max-time 3 \
    "https://pandaolate.ru:18443/$1" -o /dev/null
}
ready=0
for _ in $(seq 1 40); do
  if probe 4k > /dev/null 2>&1; then ready=1; break; fi
  sleep 0.5
done
if (( ready == 0 )); then
  tail -n 60 "$REPORT/edge-nginx.log" "$REPORT/home-nginx.log" "$REPORT/backend.log" >&2
  exit 1
fi

ip netns exec "$CLIENT" curl --noproxy '*' \
  --resolve "pandaolate.ru:18443:10.77.0.1" -kfsS --max-time 3 \
  -H "X-Real-IP: 203.0.113.77" \
  -H "X-Forwarded-For: 203.0.113.77" \
  -H "X-Forwarded-Proto: http" \
  -H "Forwarded: for=203.0.113.77;proto=http" \
  -H "X-Forwarded-Host: fake.example" \
  -H "X-Forwarded-Server: fake.example" \
  "https://pandaolate.ru:18443/headers" > "$REPORT/headers.json"
python3 - "$REPORT/headers.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    received = json.load(f)
assert received == {
    "real_ip": "10.77.0.2",
    "forwarded_for": "10.77.0.2",
    "forwarded_proto": "https",
    "forwarded": "",
    "forwarded_host": "",
    "forwarded_server": "",
}, received
PY

for path in healthz readyz metrics internal/metrics; do
  code="$(ip netns exec "$CLIENT" curl --noproxy '*' \
    --resolve "pandaolate.ru:18443:10.77.0.1" -ksS \
    -o /dev/null -w '%{http_code}' --max-time 3 \
    "https://pandaolate.ru:18443/$path")"
  if [[ "$code" != 404 ]]; then
    echo "UNSAFE diagnostic route $path returned $code" >&2
    exit 1
  fi
done

ip netns exec "$CLIENT" curl --noproxy '*' -sS --max-time 3 \
  -H 'Host: pandaolate.ru' -D "$REPORT/redirect.txt" -o /dev/null \
  "http://10.77.0.1:18080/catalog?q=test"
grep -q 'HTTP/1.1 308' "$REPORT/redirect.txt"
grep -q 'Location: https://pandaolate.ru/catalog?q=test' "$REPORT/redirect.txt"
if ip netns exec "$CLIENT" curl --noproxy '*' -sS --max-time 3 \
  -H 'Host: evil.example' "http://10.77.0.1:18080/4k" \
  -o /dev/null > /dev/null 2>&1; then
  echo 'UNSAFE: unknown HTTP Host was accepted' >&2
  exit 1
fi
if ip netns exec "$CLIENT" curl --noproxy '*' -ksS --max-time 3 \
  --resolve "evil.example:18443:10.77.0.1" \
  "https://evil.example:18443/4k" \
  -o /dev/null > /dev/null 2>&1; then
  echo 'UNSAFE: unknown TLS SNI was accepted' >&2
  exit 1
fi

for port in 3000 18081 5432; do
  if ip netns exec "$EDGE" curl --noproxy '*' -fsS --connect-timeout 1 \
    --max-time 2 "http://10.78.0.2:$port/4k" -o /dev/null \
    > /dev/null 2>&1; then
    echo "UNSAFE: home port $port is reachable over WAN" >&2
    exit 1
  fi
done

ip -n "$HOME" link set wg-home down
code="$(ip netns exec "$CLIENT" curl --noproxy '*' \
  --resolve "pandaolate.ru:18443:10.77.0.1" -ksS \
  -o /dev/null -w '%{http_code}' --max-time 8 \
  "https://pandaolate.ru:18443/4k" || true)"
case "$code" in
  502|504) ;;
  *) echo "UNSAFE: tunnel loss returned HTTP $code" >&2; exit 1 ;;
esac
ip -n "$HOME" link set wg-home up
ip netns exec "$HOME" ping -n -c 2 -W 3 10.88.0.1 > "$REPORT/reconnected.txt"
probe 4k

{
  echo 'Both future Nginx examples passed nginx -t.'
  echo 'Two-hop HTTPS and real client IP survived; spoofed headers were rejected.'
  echo 'Public diagnostics and unknown hosts/SNI were rejected.'
  echo 'WAN cannot reach home app, Nginx gateway or PostgreSQL.'
  echo 'Tunnel failure produced 502/504, then recovery restored HTTP 200.'
  echo 'Ephemeral keys and certificate were not retained.'
} > "$REPORT/summary.txt"
cat "$REPORT/summary.txt"
