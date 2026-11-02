"""Static checks for future TOR home deployment templates; never contact hosts."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SAMPLES = ROOT / "ops/home-ingress"


class HomeIngressTemplatesTests(unittest.TestCase):
    def test_no_real_secrets_or_automatic_install(self):
        for name in ("vps-wg0.conf.example", "home-wg0.conf.example"):
            source = (SAMPLES / name).read_text()
            self.assertIn("<REPLACE_WITH_", source)
            self.assertNotIn("0.0.0.0/0", "\n".join(
                line for line in source.splitlines() if not line.lstrip().startswith("#")
            ))
            self.assertNotIn("PostUp =", source)
            self.assertNotIn("PostDown =", source)

    def test_wireguard_is_narrow_and_home_initiated(self):
        edge = (SAMPLES / "vps-wg0.conf.example").read_text()
        home = (SAMPLES / "home-wg0.conf.example").read_text()
        self.assertIn("AllowedIPs = 10.88.0.2/32", edge)
        self.assertIn("AllowedIPs = 10.88.0.1/32", home)
        self.assertIn("PersistentKeepalive = 25", home)
        self.assertIn("Endpoint = <REPLACE_WITH_VPS_PUBLIC_IPV4>:51820", home)
        self.assertNotIn("\nEndpoint =", edge)

    def test_edge_strips_untrusted_headers_and_diagnostics(self):
        edge = (SAMPLES / "vps-site.nginx.conf.example").read_text()
        self.assertIn("proxy_pass http://10.88.0.2:18081;", edge)
        self.assertIn("proxy_set_header X-Forwarded-For $remote_addr;", edge)
        self.assertIn("proxy_set_header X-Real-IP $remote_addr;", edge)
        self.assertIn('proxy_set_header Forwarded "";', edge)
        self.assertIn("ssl_reject_handshake on;", edge)
        self.assertIn("proxy_buffering off;", edge)
        self.assertIn("proxy_request_buffering off;", edge)
        self.assertIn("client_max_body_size 34m;", edge)
        self.assertIn("location = /readyz { return 404; }", edge)
        self.assertIn("location = /internal/metrics { return 404; }", edge)
        self.assertNotIn("proxy_cache ", edge)
        self.assertNotIn("proxy_pass http://127.0.0.1:3000;", edge)

    def test_home_http_and_pg_are_not_public(self):
        home = (SAMPLES / "home-site.nginx.conf.example").read_text()
        self.assertIn("listen 10.88.0.2:18081;", home)
        self.assertIn("allow 10.88.0.1;", home)
        self.assertIn("deny all;", home)
        self.assertIn("proxy_pass http://127.0.0.1:3000;", home)
        self.assertNotIn("listen 0.0.0.0:", home)
        self.assertNotIn("proxy_pass http://127.0.0.1:5432;", home)
        self.assertIn("proxy_buffering off;", home)


if __name__ == "__main__":
    unittest.main()
