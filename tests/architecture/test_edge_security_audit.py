"""AB-EDGE security audit: private, read-only inventory only."""
from pathlib import Path
import json
import subprocess
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from audit_edge_security import audit, read_nginx_local, extract_servers, remove_comments

SAMPLE = ROOT / "ops/home-ingress/vps-site.nginx.conf.example"


class EdgeAuditTests(unittest.TestCase):
    def test_future_edge_sample_reports_observation_not_enforcement(self):
        report = audit(SAMPLE.read_text(encoding="utf-8"))
        self.assertEqual(report["matching_tls_vhosts"], 1)
        self.assertEqual(report["rate_limit"], "DRY_RUN")
        self.assertEqual(report["source_ip"], "DIRECT_PEER_VARIABLE")
        self.assertEqual(report["access_log"], "CONFIGURED_FORMAT_UNREVIEWED")
        self.assertEqual(report["scanner"], "PRESENT")
        self.assertEqual(report["backend"], "PRESENT_DESTINATION_UNVERIFIED")
        self.assertEqual(report["tor_auth_trust_nginx_real_ip"], "NOT_INSPECTED")

    def test_sibling_host_cannot_satisfy_site_security(self):
        other = """
server {
 listen 443 ssl;
 server_name unrelated.example;
 limit_req zone=site;
 access_log /tmp/private main;
 proxy_set_header X-Real-IP $remote_addr;
}
server {
 listen 443 ssl;
 server_name pandaolate.ru;
 access_log off;
 proxy_pass http://127.0.0.1:3000;
 proxy_set_header X-Real-IP $http_x_real_ip;
 # Secret Authorization Bearer secret_123456
 set $example "secret_123456#not-a-comment";
}
"""
        report = audit(other)
        self.assertEqual(report["rate_limit"], "ABSENT")
        self.assertEqual(report["source_ip"], "TRUST_CHAIN_REVIEW")
        self.assertEqual(report["access_log"], "OFF")
        self.assertIn("ACCESS_LOG_DISABLED", report["findings"])
        serialized = json.dumps(report)
        for raw in ("secret_123456", "/tmp/private", "127.0.0.1",
                    "unrelated.example", "pandaolate.ru"):
            self.assertNotIn(raw, serialized)

    def test_comment_only_server_cannot_fake_https(self):
        sample = "# server { listen 443 ssl; server_name pandaolate.ru; }\n"
        sample += "server { listen 80; server_name pandaolate.ru; }"
        self.assertEqual(audit(sample)["matching_tls_vhosts"], 0)
        self.assertEqual(len(extract_servers(sample)), 1)

    def test_nested_location_braces_are_not_new_servers(self):
        source = """
http {
 server {
  listen 443 ssl;
  server_name pandaolate.ru;
  location / { if ($request_method = POST) { return 405; } }
 }
 server { listen 443 ssl; server_name unrelated.example; }
}
"""
        self.assertEqual(len(extract_servers(source)), 2)
        self.assertEqual(audit(source)["matching_tls_vhosts"], 1)

    def test_comment_strip_respects_quoted_hash(self):
        self.assertIn("secret#internal", remove_comments(
            'server { set $t "secret#internal"; # public comment\n}'
        ))

    def test_malformed_and_oversized_config_not_accepted(self):
        with self.assertRaises(ValueError):
            audit("server { listen 443 ssl; server_name pandaolate.ru;")
        with self.assertRaises(ValueError):
            audit("a" * (4 * 1024 * 1024 + 1))

    def test_local_reader_never_prints_nginx_errors(self):
        with patch("audit_edge_security.subprocess.run") as mock:
            mock.return_value = subprocess.CompletedProcess(
                ["nginx", "-T"], 1, "Authorization: secret_9876",
                "fatal /private/config path")
            with self.assertRaises(ValueError) as error:
                read_nginx_local()
            self.assertNotIn("secret_9876", str(error.exception))
            self.assertNotIn("/private/config", str(error.exception))
            self.assertEqual(mock.call_args.args[0], ["nginx", "-T"])


if __name__ == "__main__":
    unittest.main()
