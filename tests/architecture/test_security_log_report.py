"""AB-EDGE: bounded security log parsing and private Nginx contracts."""
from datetime import datetime, timezone
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from security_log_report import parse_line, summarize

NOW = datetime(2026, 11, 24, 12, 0, tzinfo=timezone.utc)


def event(*, when="2026-11-24T11:59:58+00:00", ip="198.51.100.41",
          method="GET", status=404, route="other", limiter="-", rt="0.005"):
    return (f"ts={when} ip={ip} method={method} status={status} "
            f"route={route} limiter={limiter} rt={rt}\n")


class SecurityLogTests(unittest.TestCase):
    def test_bounded_window_and_no_exact_ip_in_default_report(self):
        entries = [
            event(), event(status=429, route="auth", limiter="REJECTED_DRY_RUN"),
            event(ip="2001:db8::123", status=444),
            event(ip="192.0.2.22", status=200, route="reading"),
            event(when="2026-11-24T11:00:00+00:00", status=403),
            event() + " cookie=private",
            "bad line\n",
        ]
        report = summarize(entries, now=NOW)
        self.assertEqual(report["requests"], 4)
        self.assertEqual(report["statuses"], {"200": 1, "404": 1, "429": 1, "444": 1})
        self.assertEqual(report["limiter_results"]["REJECTED_DRY_RUN"], 1)
        self.assertEqual(report["candidate_sources"][0],
                         {"source": "198.51.100.0/24", "signals": 2})
        self.assertNotIn("198.51.100.41", str(report))
        self.assertNotIn("2001:db8::123", str(report))
        self.assertEqual(report["outside_window_or_invalid_lines"], 3)

    def test_exact_ip_requires_explicit_switch(self):
        report = summarize([event()], now=NOW, show_full_ip=True)
        self.assertEqual(report["candidate_sources"][0]["source"], "198.51.100.41")

    def test_bad_log_lines_are_not_reflected(self):
        for row in [
            event().replace("route=other", "route=/private/book"),
            event().replace("status=404", "status=999"),
            event().replace("ts=2026-11-24T11:59:58+00:00", "ts=invalid"),
            event().replace("rt=0.005", "rt=nan"),
            event().replace("ip=198.51.100.41", "ip=forged"),
        ]:
            with self.subTest(row=row[:60]):
                self.assertIsNone(parse_line(row))

    def test_window_fails_closed_on_invalid_inputs(self):
        with self.assertRaises(ValueError):
            summarize([], now=NOW, minutes=0)
        with self.assertRaises(ValueError):
            summarize([], now=NOW.replace(tzinfo=None))

    def test_config_must_use_direct_peer_and_never_log_secrets(self):
        config = (ROOT / "ops/home-ingress/vps-site.nginx.conf.example").read_text()
        self.assertIn("limit_req_zone $binary_remote_addr zone=tor_public_per_ip:", config)
        self.assertIn("limit_req_zone $tor_auth_client_key zone=tor_auth_per_ip:", config)
        self.assertIn("limit_req_dry_run on;", config)
        self.assertIn("limit_req zone=tor_public_per_ip burst=80 nodelay;", config)
        self.assertIn("limit_req zone=tor_auth_per_ip burst=8 nodelay;", config)
        self.assertIn("proxy_set_header X-Real-IP $remote_addr;", config)
        self.assertIn("upstream tor_home_backend {", config)
        self.assertIn("keepalive 16;", config)
        self.assertIn("proxy_pass http://tor_home_backend;", config)
        active_log = config.split("log_format tor_security", 1)[1].split(";", 1)[0]
        for banned in ("request_uri", "$args", "$http_", "cookie", "$uri", "$request "):
            with self.subTest(banned=banned):
                self.assertNotIn(banned, active_log)


if __name__ == "__main__":
    unittest.main()
