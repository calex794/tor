# TOR: entry-only VPS + outbound WireGuard + physical home server

**Status: candidate configuration, NOT deployed or production-approved.**
Owner: integration/operations (line I). Related design:
[Notion TOR-HOME-01](https://app.notion.com/p/40832a84beec8103b8d3c43170472381).

## Purpose and trust boundaries

```text
Browser -- TCP/443 (TLS stays encrypted) --> public edge VPS (NGINX stream)
       --> WireGuard 10.77.77.1 -> 10.77.77.2:8443
       --> home NGINX (TLS certificate + PROXY protocol)
       --> localhost:3000 Rust/Axum --> localhost:5432 PostgreSQL

Browser -- direct HTTPS --> S3 object store (free objects or encrypted paid
                            objects after TOR grants entitlement)
```

- The external VPS is a **TCP relay only**. It has no TLS key, user cookies, database, paid material, or S3 encryption key.
- No public IPv4, open inbound ports, or DDNS are required on the **home** router. The home WireGuard peer initiates the tunnel to the VPS.
- The edge exposes public TCP/443 and UDP/51820 for WireGuard. Keep SSH administrative access restricted, never expose PostgreSQL or the Rust listener publicly.
- Use a dedicated home VLAN/subnet: no initiations from the public server to personal devices. Accept home TCP/8443 on the WireGuard interface **only from 10.77.77.1**. Deny other ingress, including other VPN peers. Do not enable IP forwarding; NGINX terminates the TCP tunnel as a userspace proxy.
- The domain A record must point to the external VPS. Remove any accidental AAAA record to the house; inspect historic DNS disclosures. This architecture reduces exposure of the domestic IP, **not** obligations under the Ufanet agreement.
- HTTPS **ends at home**; obtain the production certificate using **DNS-01** (public TCP/80 at home isn't required). Only TCP/443 is exposed to readers: HTTP/3/QUIC (UDP/443) and automatic HTTP-to-HTTPS on port 80 are not part of this configuration.
- A basic DDoS service at the edge may filter L3/L4 attacks; this is **not** an HTTP-level WAF. Do not describe an inexpensive VPS as comprehensive DDoS protection.

## Configurations (review only: no provisioning scripts)

All files are **complete** nginx.conf examples for a *dedicated* machine, not drop-in virtual-host snippets. The templates assume Debian/Ubuntu NGINX with `libnginx-mod-stream` and `include /etc/nginx/modules-enabled/*.conf`. Never overwrite an existing nginx.conf automatically. No deployment workflow references these files.

1. Generate distinct WireGuard key pairs **on their respective machines** using `wg genkey` and `wg pubkey`. Set `/etc/wireguard/wg0.conf` permissions to 0600. Replace all `REPLACE_WITH_...` markers locally. The edge sample listens on UDP/51820; the home sample connects to the documentation address `203.0.113.10` which must be replaced with the real VPS address. Peer AllowedIPs are /32 and must never become a default route.
2. After setting up the encrypted tunnel, confirm that the home can contact `10.77.77.1` and the edge can contact `10.77.77.2`; also verify that a third peer or the public Internet cannot connect to `10.77.77.2:8443`.
3. Render the **edge** full NGINX configuration, e.g.:

```sh
python3 tools/render_home_gateway.py edge \
  --listen 0.0.0.0:443 --home 10.77.77.2:8443 \
  --pid /run/nginx-edge.pid --output /tmp/tor-edge.nginx.conf
```

4. Render the **home** full NGINX configuration, using certificate paths only valid on the home PC:

```sh
python3 tools/render_home_gateway.py home \
  --listen 10.77.77.2:8443 --edge-ip 10.77.77.1 \
  --backend 127.0.0.1:3000 --domain pandaolate.ru \
  --cert /etc/letsencrypt/live/pandaolate.ru/fullchain.pem \
  --key /etc/letsencrypt/live/pandaolate.ru/privkey.pem \
  --pid /run/nginx-home.pid --output /tmp/tor-home.nginx.conf
```

These commands only write a file; no server is started/restarted. Once approved,
a future operator must install each reviewed file on its corresponding dedicated
host, run `nginx -t`, and separately confirm firewall, DNS, certificates, and
a tested rollback. **Do not run on the existing TOR VPS.**

## Preventing client-IP spoofing

- The edge `stream` block preserves raw TLS and **adds a PROXY v1 TCP header** reflecting the real client socket address; no client-supplied HTTP headers are trusted.
- The home listens *only* at `10.77.77.2:8443 ssl proxy_protocol`, and replaces `$remote_addr` only if the direct peer is `10.77.77.1/32` through `set_real_ip_from` and `real_ip_header proxy_protocol`.
- The home overwrites `X-Real-IP` and `X-Forwarded-For` with `$remote_addr`; it strips the untrusted `Forwarded` header, and passes `Origin` and CSRF headers without altering them.
- The Rust process binds `127.0.0.1:3000`. Enable `TOR_AUTH_TRUST_NGINX_REAL_IP=1` **only after** the trust chain and negative spoof test pass. Follow [existing login limiter contract](tor-production-activation.md). IPv6 external clients are represented in PROXY v1; the WireGuard peer restriction remains IPv4 /32.
- Avoid caching or logging request bodies, `Cookie`, paid S3 grant URLs, query secrets, or KEK/DEK. The edge does not decrypt the HTTPS stream; the home terminates TLS, so it must protect the certificate key and app secrets.

## CI validation / limits

Run on a disposable GitHub runner (or a locally provisioned Debian/Ubuntu machine):

```sh
sudo apt-get update -qq
sudo apt-get install -y nginx libnginx-mod-stream
python3 -m unittest discover -s tests/infra -p test_home_gateway.py -v
```

`TOR home gateway smoke` runs `nginx -t` on **both rendered configurations**, then starts both NGINX instances on loopback, generates a throwaway test certificate, verifies TLS passthrough, authentic HTTP to a mock backend, original-IP forwarding, hostile `X-Real-IP`/`X-Forwarded-For` header replacement, untrusted Host rejection, and refusal of raw TLS lacking a PROXY preamble. There are no real WireGuard keys or external ports in CI.

**Not tested by this job:** real WireGuard UDP reachability/NAT, failures of the ISP, provider anti-DDoS, production certificates, real Rust/PostgreSQL, application CSRF, availability of actual S3/paid grants, and sustained load. WireGuard samples have narrow AllowedIPs but must be validated in an isolated integration test before production. This PR cannot authorize go-live, real payments or a production deployment.

**S3 caveat (current main):** [public_reader_http.rs](../../crates/web/src/public_reader_http.rs) still serves free chapter body through Rust, and [cover_http.rs](../../crates/web/src/cover_http.rs) relays privately stored covers through Rust. The future direct S3 delivery must be tested separately, rather than assuming every byte bypasses the house.

Documentation: [NGINX PROXY protocol](https://docs.nginx.com/nginx/admin-guide/load-balancer/using-proxy-protocol/), [HTTP realip](https://nginx.org/en/docs/http/ngx_http_realip_module.html), [WireGuard](https://www.wireguard.com/quickstart/).
