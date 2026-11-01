"""RIL-09 safety fixtures: fingerprint, backend binding and Nginx exposure."""
from __future__ import annotations

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_ril_exposure import (  # noqa: E402
    check_nginx, check_repository, fingerprint, probe, run,
)

GOOD_NGINX = """
server {
    listen 80;
    server_name pandaolate.ru;
    return 301 https://pandaolate.ru$request_uri;
}
server {
    listen 443 ssl;
    server_name pandaolate.ru;
    ssl_certificate /etc/ssl/certs/example.pem;
    location / { proxy_pass http://127.0.0.1:3000; }
}
server {
    listen 443 ssl;
    server_name other.example;
    location / { proxy_pass http://127.0.0.1:4444; }
}
"""


class RilExposureTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        for rel in (
            "docs/contracts/ril-route-guards-v1.json",
            "ops/tor-web.service.example",
            "crates/web/src/main.rs",
        ):
            source = ROOT / rel
            target = self.root / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)

    def mutate(self, path: str, original: str, replacement: str):
        target = self.root / path
        data = target.read_text(encoding="utf-8")
        self.assertIn(original, data)
        target.write_text(data.replace(original, replacement, 1), encoding="utf-8")

    def test_current_contract_is_loopback_and_off_by_default(self):
        self.assertEqual(check_repository(self.root), [])
        report = fingerprint(self.root)
        self.assertEqual(report["route_bindings"], 165)  # Includes E-08 creator-transfer route.
        self.assertEqual(report["route_pairs"], 163)  # All current RIL route/method pairs.
        self.assertEqual(len(report["sha256"]), 64)
        audit, issues = run(self.root)
        self.assertEqual(issues, [])
        self.assertFalse(audit["nginx_config_reviewed"])
        self.assertFalse(audit["public_origin_reviewed"])

    def test_fingerprint_is_canonical_across_policy_record_order(self):
        before = fingerprint(self.root)["sha256"]
        path = self.root / "docs/contracts/ril-route-guards-v1.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        data["routes"].reverse()
        path.write_text(json.dumps(data), encoding="utf-8")
        self.assertEqual(before, fingerprint(self.root)["sha256"])

    def test_change_in_handler_or_security_changes_fingerprint(self):
        before = fingerprint(self.root)["sha256"]
        self.mutate(
            "docs/contracts/ril-route-guards-v1.json",
            '"policy": "owner"', '"policy": "session"',
        )
        self.assertNotEqual(before, fingerprint(self.root)["sha256"])

    def test_public_listener_breaks_deployment_safety(self):
        self.mutate(
            "ops/tor-web.service.example",
            "Environment=TOR_LISTEN_ADDR=127.0.0.1:3000",
            "Environment=TOR_LISTEN_ADDR=0.0.0.0:3000",
        )
        self.assertTrue(any("loopback" in x for x in check_repository(self.root)))

    def test_implicit_personal_analysis_is_rejected(self):
        self.mutate(
            "ops/tor-web.service.example",
            "Environment=TOR_AUTH_ENABLED=1",
            "Environment=TOR_AUTH_ENABLED=1\nEnvironment=TOR_RIL_SINGLE_INSTANCE=1",
        )
        self.assertTrue(any("personal path analysis" in x for x in check_repository(self.root)))

    def test_admin_endpoint_is_not_public(self):
        path = self.root / "docs/contracts/ril-route-guards-v1.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        data["routes"].append({
            "method": "GET", "template": "/admin/users", "handler": "users",
            "source": "crates/web/src/lib.rs", "policy": "public",
        })
        path.write_text(json.dumps(data), encoding="utf-8")
        self.assertTrue(any("internal/admin" in x for x in check_repository(self.root)))

    def test_exact_admin_mail_exception_cannot_be_extended(self):
        self.assertEqual(check_repository(self.root), [])
        target = self.root / "docs/contracts/ril-route-guards-v1.json"
        rows = json.loads(target.read_text())
        rows["routes"].append({
            "method": "GET", "template": "/admin/mail/export",
            "source": "crates/web/src/auth.rs",
            "handler": "admin_mail::index", "policy": "admin_mail",
        })
        target.write_text(json.dumps(rows))
        self.assertTrue(any("internal/admin" in issue for issue in check_repository(self.root)))

    def test_expected_https_nginx_server_is_safe(self):
        self.assertEqual(check_nginx(GOOD_NGINX), [])
        audit, issues = run(self.root, GOOD_NGINX)
        self.assertEqual(issues, [])
        self.assertTrue(audit["nginx_config_reviewed"])
        self.assertFalse(audit["public_origin_reviewed"])
        self.assertNotIn("proxy_pass", json.dumps(audit))

    def test_no_matching_hostname_fails_closed(self):
        self.assertTrue(any("server_name" in x for x in check_nginx(
            GOOD_NGINX.replace("pandaolate.ru", "other.invalid")
        )))

    def test_proxying_plain_http_does_not_pass(self):
        config = GOOD_NGINX.replace(
            "return 301 https://pandaolate.ru$request_uri;",
            "location / { proxy_pass http://127.0.0.1:3000; }",
        )
        self.assertTrue(any("port 80" in x for x in check_nginx(config)))

    def test_upstream_must_remain_loopback(self):
        config = GOOD_NGINX.replace(
            "proxy_pass http://127.0.0.1:3000;",
            "proxy_pass http://203.0.113.1:3000;",
        )
        self.assertTrue(any("loopback" in x for x in check_nginx(config)))

    def test_public_status_directory_listing_and_filesystem_leaks_fail(self):
        for mutation, expected in [
            ("stub_status on;", "status exposed"),
            ("autoindex on;", "directory listing"),
            ("alias /etc/;", "system filesystem"),
        ]:
            with self.subTest(mutation=mutation):
                config = GOOD_NGINX.replace(
                    "location / { proxy_pass http://127.0.0.1:3000; }",
                    "location / { proxy_pass http://127.0.0.1:3000;"
                    + mutation + " }",
                )
                self.assertTrue(any(expected in x for x in check_nginx(config)))

    def test_wrong_origin_is_rejected_without_network_access(self):
        statuses, issues = probe("https://other.example")
        self.assertEqual(statuses, {})
        self.assertTrue(any("approved TOR HTTPS" in x for x in issues))


if __name__ == "__main__":
    unittest.main()
